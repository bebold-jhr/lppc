# Code style

+ Strive for meaningful names that give context and help other understand the code better
    + Example: Instead of `let t: uint8` use something like `let duration_in_seconds: uint8`
+ Write code that is idiomatic for the respective language
+ Try to write the code based on the principles of "clean code" by "Robert C. Martin" unless it is contradicting any given instructions.

# Tests

+ Test functions well
    + Try to reach every branch by changing perspectives (whitebox test and blackbox test)
    + Don't create tests like getter/setter tests in Java. The goal is not a specific number for coverage, but to test every possibility / possible behavior that comes to mind

# Workflow

1. Understand your task before you start. If anything is unclear, contradicting or if you have other questions, stop and ask before you proceed.
2. Check the code that already exists
3. Create a plan on how to implement the changes
4. Use the skill `depcheck` whenever you plan to add new dependencies
5. Let a security-engineer subagent check the code for flaws or vulnerabilities
6. Update the @README.md or additional documentation if needed
7. Always run tests before you say that you have finished