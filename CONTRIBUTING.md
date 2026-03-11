# Contributing to Walt

Thanks for your interest in contributing! This guide will help you get started.

## Quick Start

1. **Fork and clone** the repository
2. **Install dependencies**: Rust toolchain, `hyprpaper`, `hyprctl`
3. **Build**: `cargo build --release`
4. **Test**: `cargo test`
5. **Run**: `./target/release/walt`

## Development Workflow

### Making Changes

- **Create a branch**: `git checkout -b feature/your-feature-name`
- **Make focused commits**: One logical change per commit
- **Write clear messages**: Use present tense ("Add feature" not "Added feature")
- **Test thoroughly**: Ensure `cargo test` passes and the app works in both TUI and GUI modes

### Code Style

- Run `cargo fmt` before committing
- Run `cargo clippy` and address warnings
- Follow existing patterns in the codebase
- Keep the TUI fast and keyboard-focused
- Keep the GUI native and preview-driven

### Testing

- Add tests for new functionality
- Test both TUI (`walt`) and GUI (`walt gui`) modes
- Verify Hyprland integration if possible
- Test on multiple displays if available

## Pull Request Process

1. **Update documentation** if you change behavior
2. **Add to CHANGELOG.md** under Unreleased section
3. **Ensure CI passes** (formatting, clippy, tests)
4. **Reference issues** in PR description (e.g., "Fixes #123")
5. **Be responsive** to review feedback

## Reporting Issues

When reporting bugs, please include:

- Walt version (`walt --version`)
- Rust version (`rustc --version`)
- Hyprland version
- Terminal emulator (for TUI issues)
- Steps to reproduce
- Expected vs actual behavior
- Screenshots if relevant

## Feature Requests

- Check existing issues first
- Describe the use case, not just the solution
- Consider both TUI and GUI implications

## Questions?

- Open an [issue](https://github.com/gitfudge0/walt/issues) for questions and discussions
- Join the conversation in existing issues

## License

By contributing, you agree that your contributions will be licensed under the MIT License.
