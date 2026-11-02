export type OwlOptions = {
  /** Path to the `owl` binary. Defaults to `owl` from PATH. */
  owlPathOverride?: string;
  /** Open WebUI server URL (`/api/v1` root). Sets `OPENWEBUI_BASE_URL`. */
  baseUrl?: string;
  /** API token for the server. Sets `OPENWEBUI_API_KEY`. */
  apiKey?: string;
  /**
   * Environment variables passed to the owl CLI process. When provided, the SDK
   * will not inherit variables from `process.env`.
   */
  env?: Record<string, string>;
};
