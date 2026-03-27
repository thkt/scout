# Setup Documentation

Generate a setup guide for a developer running this project locally.

## Sections

### 1. Prerequisites

Table with columns: Tool | Version | Required

List tools needed before setup (language runtime, package manager, database, etc.).
Detect versions from config files (.nvmrc, .python-version, rust-toolchain.toml, etc.).

### 2. Installation

Step-by-step commands to get the project running:

```bash
# Clone and install
git clone <repo_url>
cd <project_name>
<install_command>
```

Include any post-install steps (migrations, seed data, code generation, etc.).

### 3. Configuration

**Environment Variables**: table with Name | Description | Required | Default | Source columns.

Detect from: .env.example, .env.sample, code references (process.env, std::env, os.environ).

**Config Files**: table with File | Purpose columns.

List key settings in each config file if relevant.

### 4. Running

**Development**: the command to start the dev server/process.
**Production**: the command for production build/run.

Include the dev URL if applicable.

### 5. Testing

- **Full suite**: the command to run all tests
- **Single file/test**: how to run a specific test by name
- **Coverage**: how to run with coverage, if applicable

### 6. Common Workflows

Table with columns: Task | Command

List frequently needed commands beyond initial setup:

- Type checking
- Linting
- Database migrations
- Code generation

Omit if fewer than 2 workflows exist.

### 7. Troubleshooting

Table with columns: Issue | Solution

List common setup problems and their fixes.
Include if at least one known problem exists (from README, docs, or config file comments).

## Analysis Techniques

1. **Package manager detection**: identify the build system from manifest files in the project root
2. **Version detection**: read language version files and manifest version fields
3. **Env var discovery**: Glob for `.env.example` / `.env.sample` / `.env.template`, then Grep source code for environment variable access to cross-validate which vars are required vs optional
4. **Config deep read**: read project config files and extract key settings (ports, output dirs, feature flags). Read actual values, never assume framework defaults
5. **Script discovery**: check all applicable task runners (package.json scripts, Makefile targets, Justfile, Taskfile.yml, cargo aliases)

## Writing Guidelines

- Write for a developer setting up the project for the first time
- Every command should be copy-pasteable
- Use `file_path:line_number` references for config file locations
- When updating, verify each `file_path:line_number` reference is still accurate

## Omit Rules

- Omit Common Workflows if fewer than 2 items would appear
- Omit Troubleshooting if no known issues exist
- Never omit Prerequisites, Installation, Configuration, Running, or Testing
