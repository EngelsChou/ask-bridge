# Release policy

- The primary and release branch is `main-add-m365-copilot`.
- Every completed change must be committed and pushed to that branch.
- A release is complete only after the version tag is pushed, the GitHub Release workflow succeeds, and `install.exe` is published.
- Release `ask-bridge` before updating or releasing a dependent `ask-bridge-mcp` installer.
