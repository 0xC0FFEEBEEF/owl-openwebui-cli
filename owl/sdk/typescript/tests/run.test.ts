import fs from "node:fs";
import os from "node:os";
import path from "node:path";

import { owlExecSpy } from "./owlExecSpy";
import { describe, expect, it } from "@jest/globals";

import { Owl } from "../src/owl";

import { assistantMessage, chat, startChatTestProxy, TEST_MODEL } from "./chatProxy";

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
      const result = await thread.run("Hello, world!");

      const expectedItems = [
        {
          id: expect.any(String),
          type: "agent_message",
          text: "Hi!",
        },
      ];
      expect(result.items).toEqual(expectedItems);
      expect(result.finalResponse).toBe("Hi!");
      // owl's chat completions wire does not surface server usage; the counters
      // arrive as zeros.
      expect(result.usage).toEqual({
        cached_input_tokens: 0,
        input_tokens: 0,
        output_tokens: 0,
      });
      expect(thread.id).toEqual(expect.any(String));
    } finally {
      await close();
    }
  });

  it("sends previous items when run is called twice", async () => {
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
      await thread.run("first input");
      await thread.run("second input");

      // Check second request continues the same thread
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

  it("continues the thread when run is called twice with options", async () => {
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
      await thread.run("first input");
      await thread.run("second input");

      expect(requests.length).toBeGreaterThanOrEqual(2);
      const payload = requests[1]!.json;

      const lastUser = payload.messages.at(-1);
      expect(lastUser?.role).toBe("user");
      expect(lastUser?.content).toBe("second input");

      const assistantEntry = payload.messages.find(
        (entry: { role: string }) => entry.role === "assistant",
      );
      expect(assistantEntry).toBeDefined();
      expect(assistantEntry?.content).toBe("First response");
    } finally {
      await close();
    }
  });

  it("resumes thread by id", async () => {
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
      await originalThread.run("first input");

      const resumedThread = client.resumeThread(originalThread.id!);
      const result = await resumedThread.run("second input");

      expect(resumedThread.id).toBe(originalThread.id);
      expect(result.finalResponse).toBe("Second response");

      // The resumed owl process replays the stored history on its chat request.
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

  it("passes turn options to exec", async () => {
    const { url, close, requests } = await startChatTestProxy({
      statusCode: 200,
      responseBodies: [chat(assistantMessage("Turn options applied"))],
    });

    const { args: spawnArgs, restore } = owlExecSpy();

    try {
      const client = new Owl({ owlPathOverride: owlExecPath, baseUrl: url, apiKey: "test" });

      const thread = client.startThread({
        model: "test-model-1",
        sandboxMode: "workspace-write",
      });
      await thread.run("apply options");

      const payload = requests[0];
      expect(payload).toBeDefined();
      const json = payload!.json as { model?: string } | undefined;

      expect(json?.model).toBe("test-model-1");
      expect(spawnArgs.length).toBeGreaterThan(0);
      const commandArgs = spawnArgs[0];

      expectPair(commandArgs, ["--sandbox", "workspace-write"]);
      expectPair(commandArgs, ["--model", "test-model-1"]);
    } finally {
      restore();
      await close();
    }
  });

  it("lists server models when no model is set", async () => {
    const { url, close, requests } = await startChatTestProxy({
      statusCode: 200,
      responseBodies: [chat(assistantMessage("Model list applied"))],
    });

    try {
      const client = new Owl({ owlPathOverride: owlExecPath, baseUrl: url, apiKey: "test" });

      const thread = client.startThread();
      const result = await thread.run("which model?");

      expect(result.finalResponse).toBe("Model list applied");
      // owl takes its model from the server's own model listing.
      expect(requests[0]!.json.model).toBe(TEST_MODEL);
    } finally {
      await close();
    }
  });

  it("passes modelReasoningEffort to exec", async () => {
    const { url, close } = await startChatTestProxy({
      statusCode: 200,
      responseBodies: [chat(assistantMessage("Reasoning effort applied"))],
    });

    const { args: spawnArgs, restore } = owlExecSpy();

    try {
      const client = new Owl({ owlPathOverride: owlExecPath, baseUrl: url, apiKey: "test" });

      const thread = client.startThread({
        modelReasoningEffort: "high",
      });
      await thread.run("apply reasoning effort");

      const commandArgs = spawnArgs[0];
      expect(commandArgs).toBeDefined();
      expectPair(commandArgs, ["--config", 'model_reasoning_effort="high"']);
    } finally {
      restore();
      await close();
    }
  });

  it("passes networkAccessEnabled to exec", async () => {
    const { url, close } = await startChatTestProxy({
      statusCode: 200,
      responseBodies: [chat(assistantMessage("Network access enabled"))],
    });

    const { args: spawnArgs, restore } = owlExecSpy();

    try {
      const client = new Owl({ owlPathOverride: owlExecPath, baseUrl: url, apiKey: "test" });

      const thread = client.startThread({
        networkAccessEnabled: true,
      });
      await thread.run("test network access");

      const commandArgs = spawnArgs[0];
      expect(commandArgs).toBeDefined();
      expectPair(commandArgs, ["--config", "sandbox_workspace_write.network_access=true"]);
    } finally {
      restore();
      await close();
    }
  });

  it("passes webSearchEnabled to exec", async () => {
    const { url, close } = await startChatTestProxy({
      statusCode: 200,
      responseBodies: [chat(assistantMessage("Web search enabled"))],
    });

    const { args: spawnArgs, restore } = owlExecSpy();

    try {
      const client = new Owl({ owlPathOverride: owlExecPath, baseUrl: url, apiKey: "test" });

      const thread = client.startThread({
        webSearchEnabled: true,
        webSearchEndpoint: "http://127.0.0.1:8888/search",
      });
      await thread.run("test web search");

      const commandArgs = spawnArgs[0];
      expect(commandArgs).toBeDefined();
      expectPair(commandArgs, ["--config", 'web_search="live"']);
      expect(commandArgs).toContain('search.url="http://127.0.0.1:8888/search"');
    } finally {
      restore();
      await close();
    }
  });

  it("passes webSearchMode to exec", async () => {
    const { url, close } = await startChatTestProxy({
      statusCode: 200,
      responseBodies: [chat(assistantMessage("Web search cached"))],
    });

    const { args: spawnArgs, restore } = owlExecSpy();

    try {
      const client = new Owl({ owlPathOverride: owlExecPath, baseUrl: url, apiKey: "test" });

      const thread = client.startThread({
        webSearchMode: "cached",
        webSearchEndpoint: "http://127.0.0.1:8888/search",
      });
      await thread.run("test web search mode");

      const commandArgs = spawnArgs[0];
      expect(commandArgs).toBeDefined();
      expectPair(commandArgs, ["--config", 'web_search="cached"']);
      expect(commandArgs).toContain('search.url="http://127.0.0.1:8888/search"');
    } finally {
      restore();
      await close();
    }
  });

  it("passes webSearchEnabled false to exec", async () => {
    const { url, close } = await startChatTestProxy({
      statusCode: 200,
      responseBodies: [chat(assistantMessage("Web search disabled"))],
    });

    const { args: spawnArgs, restore } = owlExecSpy();

    try {
      const client = new Owl({ owlPathOverride: owlExecPath, baseUrl: url, apiKey: "test" });

      const thread = client.startThread({
        webSearchEnabled: false,
      });
      await thread.run("test web search disabled");

      const commandArgs = spawnArgs[0];
      expect(commandArgs).toBeDefined();
      expectPair(commandArgs, ["--config", 'web_search="disabled"']);
    } finally {
      restore();
      await close();
    }
  });

  it("passes approvalPolicy to exec", async () => {
    const { url, close } = await startChatTestProxy({
      statusCode: 200,
      responseBodies: [chat(assistantMessage("Approval policy set"))],
    });

    const { args: spawnArgs, restore } = owlExecSpy();

    try {
      const client = new Owl({ owlPathOverride: owlExecPath, baseUrl: url, apiKey: "test" });

      const thread = client.startThread({
        approvalPolicy: "on-request",
      });
      await thread.run("test approval policy");

      const commandArgs = spawnArgs[0];
      expect(commandArgs).toBeDefined();
      expectPair(commandArgs, ["--config", 'approval_policy="on-request"']);
    } finally {
      restore();
      await close();
    }
  });

  it("allows overriding the env passed to the owl CLI", async () => {
    const { url, close } = await startChatTestProxy({
      statusCode: 200,
      responseBodies: [chat(assistantMessage("Custom env"))],
    });

    const { envs: spawnEnvs, restore } = owlExecSpy();
    process.env.OWL_ENV_SHOULD_NOT_LEAK = "leak";

    // owl resolves its settings home from HOME, so a replaced env must keep
    // HOME (and PATH) while proving no other variables leak through.
    const tempHome = fs.mkdtempSync(path.join(os.tmpdir(), "owl-sdk-home-"));
    try {
      const client = new Owl({
        owlPathOverride: owlExecPath,
        baseUrl: url,
        apiKey: "test",
        env: {
          CUSTOM_ENV: "custom",
          HOME: tempHome,
          PATH: process.env.PATH ?? "",
        },
      });

      const thread = client.startThread();
      await thread.run("custom env");

      const spawnEnv = spawnEnvs[0];
      expect(spawnEnv).toBeDefined();
      if (!spawnEnv) {
        throw new Error("Spawn env missing");
      }
      expect(spawnEnv.CUSTOM_ENV).toBe("custom");
      expect(spawnEnv.OWL_ENV_SHOULD_NOT_LEAK).toBeUndefined();
      expect(spawnEnv.OPENWEBUI_BASE_URL).toBe(url);
      expect(spawnEnv.OPENWEBUI_API_KEY).toBe("test");
      expect(spawnEnv.CODEX_INTERNAL_ORIGINATOR_OVERRIDE).toBeDefined();
    } finally {
      delete process.env.OWL_ENV_SHOULD_NOT_LEAK;
      fs.rmSync(tempHome, { recursive: true, force: true });
      restore();
      await close();
    }
  });

  it("passes additionalDirectories as repeated flags", async () => {
    const { url, close } = await startChatTestProxy({
      statusCode: 200,
      responseBodies: [chat(assistantMessage("Additional directories applied"))],
    });

    const { args: spawnArgs, restore } = owlExecSpy();

    try {
      const client = new Owl({ owlPathOverride: owlExecPath, baseUrl: url, apiKey: "test" });

      const thread = client.startThread({
        additionalDirectories: ["../backend", "/tmp/shared"],
      });
      await thread.run("test additional dirs");

      const commandArgs = spawnArgs[0];
      expect(commandArgs).toBeDefined();
      if (!commandArgs) {
        throw new Error("Command args missing");
      }

      // Find the --add-dir flags
      const addDirArgs: string[] = [];
      for (let i = 0; i < commandArgs.length; i += 1) {
        if (commandArgs[i] === "--add-dir") {
          addDirArgs.push(commandArgs[i + 1] ?? "");
        }
      }
      expect(addDirArgs).toEqual(["../backend", "/tmp/shared"]);
    } finally {
      restore();
      await close();
    }
  });

  it("combines structured text input segments", async () => {
    const { url, close, requests } = await startChatTestProxy({
      statusCode: 200,
      responseBodies: [chat(assistantMessage("Combined input applied"))],
    });

    try {
      const client = new Owl({ owlPathOverride: owlExecPath, baseUrl: url, apiKey: "test" });

      const thread = client.startThread();
      await thread.run([
        { type: "text", text: "Describe file changes" },
        { type: "text", text: "Focus on impacted tests" },
      ]);

      const payload = requests[0];
      expect(payload).toBeDefined();
      const lastUser = payload!.json.messages.at(-1);
      expect(lastUser?.content).toBe("Describe file changes\n\nFocus on impacted tests");
    } finally {
      await close();
    }
  });

  it("forwards images to exec", async () => {
    const { url, close } = await startChatTestProxy({
      statusCode: 200,
      responseBodies: [chat(assistantMessage("Images applied"))],
    });

    const { args: spawnArgs, restore } = owlExecSpy();
    const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), "owl-sdk-images-"));
    const imagesDirectoryEntries: [string, string] = [
      path.join(tempDir, "first.png"),
      path.join(tempDir, "second.jpg"),
    ];
    imagesDirectoryEntries.forEach((image, index) => {
      fs.writeFileSync(image, `image-${index}`);
    });

    try {
      const client = new Owl({ owlPathOverride: owlExecPath, baseUrl: url, apiKey: "test" });

      const thread = client.startThread();
      await thread.run([
        { type: "text", text: "describe the images" },
        { type: "local_image", path: imagesDirectoryEntries[0] },
        { type: "local_image", path: imagesDirectoryEntries[1] },
      ]);

      const commandArgs = spawnArgs[0];
      expect(commandArgs).toBeDefined();
      const forwardedImages: string[] = [];
      for (let i = 0; i < commandArgs!.length; i += 1) {
        if (commandArgs![i] === "--image") {
          forwardedImages.push(commandArgs![i + 1] ?? "");
        }
      }
      expect(forwardedImages).toEqual(imagesDirectoryEntries);
    } finally {
      fs.rmSync(tempDir, { recursive: true, force: true });
      restore();
      await close();
    }
  });

  it("runs in provided working directory", async () => {
    const { url, close } = await startChatTestProxy({
      statusCode: 200,
      responseBodies: [chat(assistantMessage("Working directory applied"))],
    });

    const { args: spawnArgs, restore } = owlExecSpy();

    try {
      const workingDirectory = fs.mkdtempSync(path.join(os.tmpdir(), "owl-sdk-working-dir-"));
      const client = new Owl({
        owlPathOverride: owlExecPath,
        baseUrl: url,
        apiKey: "test",
      });

      const thread = client.startThread({
        workingDirectory,
        skipGitRepoCheck: true,
      });
      await thread.run("use custom working directory");

      const commandArgs = spawnArgs[0];
      expectPair(commandArgs, ["--cd", workingDirectory]);
    } finally {
      restore();
      await close();
    }
  });

  it("throws if working directory is not git and no skipGitRepoCheck is provided", async () => {
    const { url, close } = await startChatTestProxy({
      statusCode: 200,
      responseBodies: [chat(assistantMessage("Working directory applied"))],
    });

    try {
      const workingDirectory = fs.mkdtempSync(path.join(os.tmpdir(), "owl-sdk-working-dir-"));
      const client = new Owl({
        owlPathOverride: owlExecPath,
        baseUrl: url,
        apiKey: "test",
      });

      const thread = client.startThread({
        workingDirectory,
      });
      await expect(thread.run("use custom working directory")).rejects.toThrow(
        /Not inside a trusted directory/,
      );
    } finally {
      await close();
    }
  });

  it("sets the owl sdk originator header", async () => {
    const { url, close, requests } = await startChatTestProxy({
      statusCode: 200,
      responseBodies: [chat(assistantMessage("Hi!"))],
    });

    try {
      const client = new Owl({ owlPathOverride: owlExecPath, baseUrl: url, apiKey: "test" });

      const thread = client.startThread();
      await thread.run("Hello, originator!");

      expect(requests.length).toBeGreaterThan(0);
      const originatorHeader = requests[0]!.headers["originator"];
      if (Array.isArray(originatorHeader)) {
        expect(originatorHeader).toContain("owl_sdk_ts");
      } else {
        expect(originatorHeader).toBe("owl_sdk_ts");
      }
    } finally {
      await close();
    }
  });

  it("throws when the server rejects the chat request", async () => {
    // 400 is not retryable, so owl fails fast (500 would burn the stream
    // retry budget with backoff first).
    const { url, close } = await startChatTestProxy({
      statusCode: 400,
      responseBodies: [],
    });

    try {
      const client = new Owl({ owlPathOverride: owlExecPath, baseUrl: url, apiKey: "test" });
      const thread = client.startThread();
      // owl surfaces the server error as a failed turn.
      await expect(thread.run("fail")).rejects.toThrow(/chat proxy failure/);
    } finally {
      await close();
    }
  }, 20000);
});

function expectPair(args: string[] | undefined, pair: [string, string]) {
  if (!args) {
    throw new Error("Args is undefined");
  }
  const index = args.indexOf(pair[0]);
  if (index === -1) {
    throw new Error(`Pair ${pair[0]} not found in args`);
  }
  expect(args[index + 1]).toBe(pair[1]);
}
