import { OwlOptions } from "./owlOptions";
import { OwlExec } from "./exec";
import { Thread } from "./thread";
import { ThreadOptions } from "./threadOptions";

/**
 * Owl is the main class for interacting with the owl coding agent.
 *
 * Use the `startThread()` method to start a new thread or `resumeThread()` to resume a previously started thread.
 */
export class Owl {
  private exec: OwlExec;
  private options: OwlOptions;

  constructor(options: OwlOptions = {}) {
    this.exec = new OwlExec(options.owlPathOverride, options.env);
    this.options = options;
  }

  /**
   * Starts a new conversation with an agent.
   * @returns A new thread instance.
   */
  startThread(options: ThreadOptions = {}): Thread {
    return new Thread(this.exec, this.options, options);
  }

  /**
   * Resumes a conversation with an agent based on the thread id.
   * Threads are persisted in ~/.owl/sessions.
   *
   * @param id The id of the thread to resume.
   * @returns A new thread instance.
   */
  resumeThread(id: string, options: ThreadOptions = {}): Thread {
    return new Thread(this.exec, this.options, options, id);
  }
}
