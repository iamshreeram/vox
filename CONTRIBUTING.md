# Contributing to Vox

Vox is a personal open-source project maintained in the [Vox GitHub repository](https://github.com/iamshreeram/vox). Contributions should be focused, tested, privacy-conscious, and consistent with the repository architecture.

## Before changing code

- Review current issues, pull requests, and phase documents in the Vox repository.
- Read [`AGENTS.md`](AGENTS.md) and relevant source/specifications before implementation.
- For new behavior, agree on scope first; policies from unrelated projects do not apply to Vox.

## Git identity and personal-repository guardrail

Before committing, set and verify the personal Git identity:

```bash
git setlive
git config user.name
git config user.email
git var GIT_AUTHOR_IDENT
git var GIT_COMMITTER_IDENT
```

Expected identity: `Shreeram <shreeram.v@live.com>`. Before pushing, inspect outgoing commits:

```bash
git log origin/$(git branch --show-current)..HEAD --format='%h %an <%ae> | %cn <%ce> %s'
```

Do not push commits with another identity. Never rewrite another contributor's commits. If rewriting a personal branch already pushed, coordinate with anyone depending on it and use `--force-with-lease`.

Do not include employer branding or internal company references, workplace-specific agents/tools, corporate usernames/emails, internal package registries, or internal host instructions in Vox code, documentation, templates, commit messages, or generated assets. Use Vox branding in user-facing content. Keep historical identifiers only where code or user data requires them; label them as compatibility identifiers and do not present them as current branding. Search changed content before committing.

## Development workflow

1. Use a focused branch or an isolated `.worktrees/` worktree.
2. Follow TDD for behavior changes: write a failing test, verify the expected failure, implement the smallest change, and rerun tests.
3. Run relevant checks before committing:

   ```bash
   bun run lint
   bun run format:check
   cargo test --manifest-path src-tauri/Cargo.toml
   cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
   ```

4. Use conventional commit prefixes and explain why the change is needed.
5. Inspect status, author/committer identity, and the outgoing commit range before pushing.

## Pull requests

AI assistants must not open or submit pull requests for this repository. Prepare the branch and an accurate summary; Ram submits the PR. Do not invent human-written text or claim a PR was opened when it was not.

Describe the problem, behavior change, tests actually run, compatibility implications, and known limitations. Do not claim releases, websites, package listings, or integrations exist unless verified.

## Code principles

- Prefer small, cohesive changes; follow DRY and YAGNI.
- Handle errors explicitly; avoid production `unwrap()`/`expect()`.
- Keep user-facing text in i18next translations.
- Keep dictated text, credentials, and personal data out of logs and commits.
- Preserve persisted settings and compatibility identifiers unless a migration is included.
