# Claude Development Instructions

## Git Commit Guidelines

This project follows Linux kernel style commit messages:

- Use imperative mood, present tense (e.g., "add feature" not "added feature")
- Keep subject line to ~50 characters
- No type prefixes (no "feat:", "fix:", "docs:", etc.)
- Body text should wrap at 72 characters if needed
- Separate subject from body with a blank line

Examples:
```
add notification on daemon shutdown

Send desktop notification when daemon exits gracefully or via kill
request. Uses 1.5 second timeout to ensure user sees the message.
```

```
expose sponsorblock options in config

Add sponsorblock_mark and sponsorblock_remove fields to Config struct.
Allows users to customize which sponsor segments are marked or removed.
Defaults to mark=all, remove=sponsor,interaction.
```

## Cargo Commands

This project uses custom cargo commands. Use these commands instead of the standard cargo commands:

- `cargo lbuild` instead of `cargo build`
- `cargo lcheck` instead of `cargo check`
- `cargo lrun` instead of `cargo run`

These suppress warnings until the code compiles.
