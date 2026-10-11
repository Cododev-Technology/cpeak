# Contributing

## Table of Contents

- [Getting Started](#getting-started)
- [Commit Messages](#commit-messages)
- [Code Style](#code-style)
- [Versioning](#versioning)
- [Before Opening a PR](#before-opening-a-pr)

---

## Getting Started

This repository holds both versions of Cpeak: the Node.js package is in `node/`, and the Rust crate is in `rust/`.

### Cpeak for Node.js

1. Clone the repository and install dependencies:

```
git clone https://github.com/Cododev-Technology/cpeak.git
cd cpeak/node
npm install
```

2. Build the project:

```
npm run build
```

3. Run the tests:

```
npm test
```

### Testing Against a Real Project

For testing your changes against an actual application, you can link your local cpeak build to any Node.js project using `npm link`:

In the `node` directory, start the build in watch mode and register the package globally:

```
npm run dev
npm link
```

Then in your test project, link to your local cpeak instead of the published version:

```
npm link cpeak
```

Any changes you make in cpeak will be rebuilt automatically and reflected in your test project immediately.

### Cpeak for Rust

Clone the repository, then build the crate and run its tests:

```
git clone https://github.com/Cododev-Technology/cpeak.git
cd cpeak/rust
cargo test
```

---

## Commit Messages

This project follows [Conventional Commits](https://www.conventionalcommits.org/).

Format:

```
<type>: <short summary>
```

- Use the imperative, present tense ("add", "fix", "update" — not "added", "fixes")
- Lowercase after the colon, no trailing period
- Keep the summary line under ~72 characters
- Add a blank line followed by a body if more detail is needed

### Types

| Type       | Use for                                                                                                  |
| ---------- | -------------------------------------------------------------------------------------------------------- |
| `feat`     | A new feature                                                                                            |
| `fix`      | A bug fix                                                                                                |
| `docs`     | Documentation only changes                                                                               |
| `style`    | Changes that don't affect code meaning (formatting, whitespace, missing semicolons, etc.)                |
| `refactor` | A code change that neither fixes a bug nor adds a feature                                                |
| `perf`     | A code change that improves performance                                                                  |
| `test`     | Adding or correcting tests                                                                               |
| `build`    | Changes to the build system or external dependencies (npm, yarn, webpack, etc.)                          |
| `ci`       | Changes to CI configuration files and scripts                                                            |
| `chore`    | Other changes that don't modify source or test files (tooling, configs, scripts)                         |
| `revert`   | Reverts a previous commit                                                                                |
| `security` | Security fixes or hardening (vulnerability patches, stronger crypto, dependency upgrades for CVEs, etc.) |

### Mixed Changes

Prefer one type per commit. Split unrelated changes (e.g. a bug fix and a UI tweak) into separate commits. If splitting isn't practical, pick the type that represents the commit's primary intent and mention the secondary change in the body:

```
fix: correct broken link redirect on mobile

Also adjusts dashboard spacing to fit the corrected layout.
```

### Examples

```
feat: add support for brotli compression
fix: preserve query string when matching routes with params
docs: document compression options in README
refactor: extract path variable parsing into its own method
perf: move request handler internals to class private methods
test: cover edge cases for routes with multiple middlewares
build: upgrade typescript to 5.5
ci: run tests on pull requests
chore: add .editorconfig
revert: revert "feat: add support for brotli compression"
security: sanitize file path in sendFile to prevent directory traversal
```

---

## Code Style

The Node.js code uses [Prettier](https://prettier.io/) for formatting. Before submitting a PR, make sure your changes are formatted (in the `node` directory):

```
npm run format
```

The Rust code uses rustfmt (in the `rust` directory):

```
cargo fmt
```

---

## Versioning

Please do not modify `package.json`, `package-lock.json` or `Cargo.toml` to bump the version. Version bumps are handled by the maintainers as part of the release process.

---

## Before Opening a PR

If your change is significant, a new feature, a breaking change, or anything that touches core behaviour, please open an issue and discuss it first before investing time in a pull request. This keeps everyone on the same page and avoids the frustration of a PR being rejected after a lot of work has gone into it.
