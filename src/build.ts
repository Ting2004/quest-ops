// Build identity. Every build is a test build unless the official release
// script marked it, so an unmarked build can never pass for a release.

export const isOfficialRelease = import.meta.env.VITE_OFFICIAL_RELEASE === "1";

/** Short commit hash of a test build ("dev" under `tauri dev`), or "". */
export const buildCommit = import.meta.env.DEV ? "dev" : (import.meta.env.VITE_BUILD_COMMIT ?? "");
