import path from "node:path";

export function owlPathOverride() {
  return (
    process.env.OWL_EXECUTABLE ??
    path.join(process.cwd(), "..", "..", "codex-rs", "target", "debug", "owl")
  );
}
