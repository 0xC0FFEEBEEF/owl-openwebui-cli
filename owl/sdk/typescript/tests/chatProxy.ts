import http from "node:http";

// A stand-in for an Open WebUI server: serves the chat completions endpoints
// owl talks to (`GET /api/v1/models`, `POST /api/v1/chat/completions`).
//
// owl's chat wire never reports token usage (the stream completes without a
// usage payload), so the SSE fixtures below carry no usage either.

export const TEST_MODEL = "owl-sdk-test-model";

export type ChatChunk = {
  choices: Array<{
    index?: number;
    delta?: { role?: string; content?: string };
    finish_reason?: string | null;
  }>;
};

export type ChatResponseBody = {
  kind: "sse";
  chunks: ChatChunk[];
  /** Keep the SSE stream open (no [DONE], no end) until the proxy is closed. */
  openEnded?: boolean;
};

export type ChatProxyOptions = {
  responseBodies: Generator<ChatResponseBody> | ChatResponseBody[];
  statusCode?: number;
};

export type ChatProxy = {
  url: string;
  close: () => Promise<void>;
  requests: RecordedRequest[];
};

export type ChatApiRequest = {
  model?: string;
  messages: Array<{
    role: string;
    content?: string | unknown;
  }>;
};

export type RecordedRequest = {
  body: string;
  json: ChatApiRequest;
  headers: http.IncomingHttpHeaders;
};

export async function startChatTestProxy(options: ChatProxyOptions): Promise<ChatProxy> {
  function* createGenerator(array: ChatResponseBody[]): Generator<ChatResponseBody> {
    for (const elem of array) {
      yield elem;
    }
    throw new Error("not enough responses provided");
  }

  const responseBodies: Generator<ChatResponseBody> = Array.isArray(options.responseBodies)
    ? createGenerator(options.responseBodies)
    : options.responseBodies;

  const requests: RecordedRequest[] = [];
  const openEndedResponses = new Set<http.ServerResponse>();

  function readRequestBody(req: http.IncomingMessage): Promise<string> {
    return new Promise<string>((resolve, reject) => {
      const chunks: Buffer[] = [];
      req.on("data", (chunk) => {
        chunks.push(typeof chunk === "string" ? Buffer.from(chunk) : chunk);
      });
      req.on("end", () => {
        resolve(Buffer.concat(chunks).toString("utf8"));
      });
      req.on("error", reject);
    });
  }

  const server = http.createServer((req, res) => {
    async function handle(): Promise<void> {
      if (req.method === "GET" && req.url === "/api/v1/models") {
        res.statusCode = 200;
        res.setHeader("content-type", "application/json");
        res.end(JSON.stringify({ data: [{ id: TEST_MODEL, object: "model" }] }));
        return;
      }

      if (req.method === "POST" && req.url === "/api/v1/chat/completions") {
        const body = await readRequestBody(req);
        const json = JSON.parse(body);
        requests.push({ body, json, headers: { ...req.headers } });

        const status = options.statusCode ?? 200;
        res.statusCode = status;
        if (status !== 200) {
          res.setHeader("content-type", "application/json");
          res.end(JSON.stringify({ error: { message: "chat proxy failure (test)" } }));
          return;
        }

        res.setHeader("content-type", "text/event-stream");
        const responseBody = responseBodies.next().value;
        for (const chunk of responseBody.chunks) {
          res.write(`data: ${JSON.stringify(chunk)}\n\n`);
        }
        if (responseBody.openEnded) {
          // Leave the stream open: owl keeps streaming events (item updates)
          // until its process is aborted.
          openEndedResponses.add(res);
          res.on("close", () => openEndedResponses.delete(res));
          return;
        }
        res.write("data: [DONE]\n\n");
        res.end();
        return;
      }

      res.statusCode = 404;
      res.end();
    }

    handle().catch(() => {
      res.statusCode = 500;
      res.end();
    });
  });

  const url = await new Promise<string>((resolve, reject) => {
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      if (!address || typeof address === "string") {
        reject(new Error("Unable to determine proxy address"));
        return;
      }
      const info = address;
      resolve(`http://${info.address}:${info.port}/api/v1`);
    });
    server.once("error", reject);
  });

  async function close(): Promise<void> {
    for (const res of openEndedResponses) {
      res.destroy();
    }
    openEndedResponses.clear();
    await new Promise<void>((resolve, reject) => {
      server.close((err) => {
        if (err) {
          reject(err);
          return;
        }
        resolve();
      });
    });
  }
  return { url, close, requests };
}

export function sse(...chunks: ChatChunk[]): ChatResponseBody {
  return {
    kind: "sse",
    chunks,
  };
}

export function chatStarted(): ChatChunk {
  return {
    choices: [{ index: 0, delta: { role: "assistant" } }],
  };
}

export function assistantMessage(text: string): ChatChunk {
  return {
    choices: [{ index: 0, delta: { content: text } }],
  };
}

export function chatCompleted(): ChatChunk {
  return {
    choices: [{ index: 0, delta: {}, finish_reason: "stop" }],
  };
}

export function chat(...chunks: ChatChunk[]): ChatResponseBody {
  return sse(chatStarted(), ...chunks, chatCompleted());
}

/** A turn that keeps streaming without ever completing. */
export function openChat(...chunks: ChatChunk[]): ChatResponseBody {
  return {
    kind: "sse",
    chunks: [chatStarted(), ...chunks],
    openEnded: true,
  };
}
