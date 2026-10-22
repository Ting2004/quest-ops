/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** "1" only in builds made by scripts/release.ps1. */
  readonly VITE_OFFICIAL_RELEASE?: string;
  /** Short commit hash the build came from, "+changes" if the tree was dirty. */
  readonly VITE_BUILD_COMMIT?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
