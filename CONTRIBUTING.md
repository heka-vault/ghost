# Contributing to Ghost

Thanks for trying Ghost. It is young, and a report from a machine that isn't its author's is
worth a lot.

## Filing an issue

Open an issue on GitHub. Say what you did, what you expected, and what happened, and include:

- **Your machine:** the distribution and version, the desktop (GNOME, KDE, …; X11 or Wayland),
  the CPU and RAM, and the microphone and speakers (a headset, a laptop's own, a USB
  microphone; with or without `make echo-cancel`).
- **The commit:** `git rev-parse --short HEAD`, or `target/debug/ghostd version`.
- **`make check`** (or `target/debug/ghostd check`): the models, the model connection and the
  apps. It prints names and addresses, never a password, but read it before you paste it:
  an app's address or a path may say more than you want.
- **For a slow reply:** `make latency`, which shows where each recent turn spent its time, and
  `target/debug/ghostd llm test` for the model connection.
- **For a problem with hearing** (the wake word not firing, words misheard, Ghost cutting
  itself off): run `GHOST_DEBUG_AUDIO=1 make daemon` and reproduce it. That log contains **what
  you said, word for word, including before the wake word**: it is written only to your
  terminal, and you should read it and cut anything private before sharing a line of it. Never
  attach recordings of other people.
- **The terminal output** of `make dev` around the problem.

Ghost writes no crash reports and sends nothing anywhere on its own.

## Changing Ghost

`main` is protected: every change reaches it through a pull request, and a pull request merges
only when the `ci` workflow passes and the branch is up to date with `main`.

1. Fork the repository, or ask for access, and work on a branch.
2. Read [CLAUDE.md](CLAUDE.md)'s Rules and the ADRs in [docs/adr/](docs/adr/) for the area you
   are changing. They are short, and they are the rules: idle audio stays on the machine, every
   change an agent makes goes through the gate, passwords live in the vault, no undo.
3. A bug fix starts with a failing test. A change made for speed is measured before and after
   (`make perf`, `make perf-micro`) and the numbers go in the pull request.
4. Run `make ci` before you push (it needs `cargo install --locked cargo-nextest cargo-deny`).
   It is everything CI runs but the bundle.
5. Write a [conventional commit](https://www.conventionalcommits.org/) message
   (`fix(voice): …`, `feat(desktop): …`, `docs: …`), and say in the pull request what you
   changed, why, and how you checked it.

When behaviour a doc or an ADR describes changes, change the doc in the same pull request.

## Working without a microphone

- The UI runs against a built-in mock: `make desktop-web`, then
  `http://localhost:1420/?mock=busy` (scenarios in `apps/desktop/src/design/mock/scenarios.ts`).
- The daemon answers typed turns: `make start`, then `make say TEXT="hello"`.
- `make perf` runs scripted spoken conversations through the real pipeline with no microphone,
  no speakers and no model tokens.

## Licence

By contributing you agree that your contribution is licensed under the repository's
[MIT licence](LICENSE).
