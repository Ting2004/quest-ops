# Quest Ops

A desktop app that turns large to-do items into short, timed objectives and
rewards finishing them. An OpenAI model breaks tasks down, scores their
priority on an Eisenhower matrix, plans the day and helps when you are stuck.

Built with Tauri 2 (Rust + SQLite backend) and React + TypeScript. Interface
in English and Simplified Chinese.

## Features

- **Intake**: add tasks, have the AI break them into 10 to 20 minute
  objectives, review and edit, deploy one or many at once.
- **Operations**: four-quadrant priority matrix. The AI scores importance and
  urgency; deadlines raise urgency automatically within your chosen window.
  Drag cards to override; the AI learns from your corrections.
- **Focus timer**: countdown per objective with limited pauses, desktop
  notifications, tray countdown, and an overtime protocol (extend, split, keep going).
- **Rewards**: XP with on-time and Strategic Ops bonuses, critical hits,
  levels, streaks (one rest day allowed) and achievements.
- **Today**: next objective per operation, AI day planning, completed work and
  an end-of-day debrief.
- **Records**: XP history, on-time rate, estimate accuracy per category.

## Development

Requirements: Node.js 18+, Rust (MSVC toolchain), Microsoft C++ Build Tools, WebView2.

```sh
npm install
npm run tauri dev                      # run with hot reload
cd src-tauri && cargo test             # backend tests
npm run tauri build -- --no-bundle     # release exe in src-tauri/target/release
```

## Data and secrets

- Database: `%APPDATA%\com.motivation.questops\questops.db`
- The OpenAI API key is stored in Windows Credential Manager, never in files.
