import path from "node:path";

import { describe, expect, it } from "@jest/globals";

import { Owl } from "../src/owl";
import { ThreadEvent } from "../src/index";

import { assistantMessage, chat, startChatTestProxy } from "./chatProxy";

const owlExecPath = path.join(process.cwd(), "..", "..", "codex-rs", "target", "debug", "owl");

describe("Owl", () => {
  it("returns thread events", async () => {
    const { url, close } = await startChatTestProxy({
      statusCode: 200,
      responseBodies: [chat(assistantMessage("Hi!"))],
    });

    try {
      const client = new Owl({ owlPathOverride: owlExecPath, baseUrl: url, apiKey: "test" });

      const thread = client.startThread();
      const result = await thread.runStreamed("Hello, world!");

      const events: ThreadEvent[] = [];
      for await (const event of result.events) {
        events.push(event);
      }

      expect(events).toEqual([
        {
          type: "thread.started",
          thread_id: expect.any(String),
        },
        {
          type: "turn.started",
        },
        {
          type: "item.completed",
          item: {
            id: expect.any(String),
            type: "agent_message",
            text: "Hi!",
          },
        },
        {
          // owl's chat completions wire does not surface server usage; the
          // counters arrive as zeros.
          type: "turn.completed",
          usage: {
            cached_input_tokens: 0,
            input_tokens: 0,
            output_tokens: 0,
          },
        },
      ]);
      expect(thread.id).toEqual(expect.any(String));
    } finally {
      await close();
    }
  });

  it("resumes thread by id when streaming", async () => {
    const { url, close, requests } = await startChatTestProxy({
      statusCode: 200,
      responseBodies: [
        chat(assistantMessage("First response")),
        chat(assistantMessage("Second response")),
      ],
    });

    try {
      const client = new Owl({ owlPathOverride: owlExecPath, baseUrl: url, apiKey: "test" });

      const originalThread = client.startThread();
      const first = await originalThread.runStreamed("first input");
      await drainEvents(first.events);

      const resumedThread = client.resumeThread(originalThread.id!);
      const second = await resumedThread.runStreamed("second input");
      await drainEvents(second.events);

      expect(resumedThread.id).toBe(originalThread.id);

      expect(requests.length).toBeGreaterThanOrEqual(2);
      const payload = requests[1]!.json;
      const assistantEntry = payload.messages.find(
        (entry: { role: string }) => entry.role === "assistant",
      );
      expect(assistantEntry?.content).toBe("First response");
      const lastUser = payload.messages.at(-1);
      expect(lastUser?.role).toBe("user");
      expect(lastUser?.content).toBe("second input");
    } finally {
      await close();
    }
  });

  it("sends previous items when runStreamed is called twice", async () => {
    const { url, close, requests } = await startChatTestProxy({
      statusCode: 200,
      responseBodies: [
        chat(assistantMessage("First response")),
        chat(assistantMessage("Second response")),
      ],
    });

    try {
      const client = new Owl({ owlPathOverride: owlExecPath, baseUrl: url, apiKey: "test" });

      const thread = client.startThread();
      const first = await thread.runStreamed("first input");
      await drainEvents(first.events);

      const second = await thread.runStreamed("second input");
      await drainEvents(second.events);

      expect(requests.length).toBeGreaterThanOrEqual(2);
      const secondRequest = requests[1];
      expect(secondRequest).toBeDefined();
      const payload = secondRequest!.json;

      const assistantEntry = payload.messages.find(
        (entry: { role: string }) => entry.role === "assistant",
      );
      expect(assistantEntry).toBeDefined();
      expect(assistantEntry?.content).toBe("First response");
    } finally {
      await close();
    }
  });
});

async function drainEvents(events: AsyncGenerator<ThreadEvent>): Promise<void> {
  let done = false;
  do {
    done = (await events.next()).done ?? false;
  } while (!done);
}
