# Code style

+ Strive for meaningful names that give context and help other understand the code better
    + Example: Instead of `let t: uint8` use something like `let duration_in_seconds: uint8`
+ Write code that is idiomatic for the respective language
+ Try to write the code based on the principles of "clean code" by "Robert C. Martin" unless it is contradicting any given instructions.

# Design principles

+ **Testability**: Design code so it can be tested without external dependencies (network, filesystem, etc.). Use dependency injection (function parameters, traits) to make behavior substitutable in tests.
+ **Loose coupling**: Keep modules independent. New modules should have minimal dependencies on existing ones. Avoid circular dependencies. Follow the existing dependency flow documented in `specs/architecture.md`.
+ **Built to change**: Write code that is easy to evolve and extend. Prefer designs that allow adding new variants (e.g., a new provider, a new block type) without modifying unrelated code.
+ **Abstractions for third-party APIs**: Do not couple business logic directly to external libraries (HTTP clients, YAML parsers, etc.). Wrap third-party calls behind functions or traits so they can be replaced or mocked.
+ **Design patterns**: Apply established patterns (strategy, factory, repository, etc.) where they naturally fit, but do not over-engineer. Follow patterns already established in the codebase.

# Tests

+ Test functions well
    + Try to reach every branch by changing perspectives (whitebox test and blackbox test)
    + Don't create tests like getter/setter tests in Java. The goal is not a specific number for coverage, but to test every possibility / possible behavior that comes to mind

# Workflow

1. Understand your task before you start. If anything is unclear, contradicting or if you have other questions, stop and ask before you proceed.
2. Check the code that already exists
    + Start by reading the relevant project's `specs/architecture.md` to understand the codebase structure, patterns, and conventions
    + Then read only the specific source files you need to modify -- do NOT scan the entire codebase
3. Create a plan on how to implement the changes. Apply the design principles above when making structural decisions.
4. Implement the changes.
5. Let a `security-reviewer` subagent check the code for flaws or vulnerabilities
6. Always run tests before you say that you have finished
7. Update the @README.md or additional documentation if needed
8. Update the relevant project's `specs/architecture.md` to reflect the changes.