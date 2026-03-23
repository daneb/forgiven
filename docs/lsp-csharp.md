# C# / .NET LSP Setup

This guide covers setting up `csharp-ls` as a language server in **forgiven**
for C# navigation features: go-to-definition, find-references, and document
symbols.

---

## What you get

| Feature | Keybinding | Notes |
|---------|-----------|-------|
| Go to definition | `SPC l d` | Jumps directly for a single result; shows a list for multiple |
| Find references | `SPC l f` | Opens a scrollable list of all usages |
| Document symbols | `SPC l s` | Shows all classes, methods, fields, and properties in the current file |
| Diagnostics | Inline gutter `●` | Errors and warnings as you edit |
| Inline completions | Ghost text | Via GitHub Copilot (separate from the LSP) |

---

## Prerequisites

- .NET SDK 6, 7, 8, or 9 installed (`dotnet --version` to verify)
- A C# project with a `.sln` or `.csproj` file

---

## Installing csharp-ls

### Option A — dotnet tool (simplest, if it works)

```sh
dotnet tool install --global csharp-ls
```

This installs `csharp-ls` to `~/.dotnet/tools/csharp-ls`. If it fails with
a `DotnetToolSettings.xml` packaging error (a known NuGet issue on .NET 9),
use Option B.

**Config:**

```toml
[[lsp.servers]]
language = "csharp"
command  = "csharp-ls"
args     = []
```

### Option B — build from source (works on all SDK versions)

```sh
git clone https://github.com/razzmatazz/csharp-language-server
cd csharp-language-server
dotnet publish src/CSharpLanguageServer -c Release -o ./out
```

This produces `./out/CSharpLanguageServer.dll`. Use an absolute path in the
config so forgiven can find it regardless of the working directory.

**Config:**

```toml
[[lsp.servers]]
language = "csharp"
command  = "dotnet"
args     = ["/absolute/path/to/csharp-language-server/out/CSharpLanguageServer.dll"]
```

---

## Config file location

`~/.config/forgiven/config.toml`

A minimal complete example:

```toml
[agent]
spec_framework = "none"

[[lsp.servers]]
language = "csharp"
command  = "dotnet"
args     = ["/Users/you/tools/csharp-ls/out/CSharpLanguageServer.dll"]
```

---

## Project layout

forgiven detects C# workspaces by looking for a `.sln` or `.csproj` file in
the directory you launch from, or in any immediate subdirectory. Both flat and
solution-style layouts are supported:

```
my-project/               ← launch forgiven from here
├── MyApp.sln
├── src/
│   ├── MyApp.Api/
│   │   ├── MyApp.Api.csproj
│   │   └── Program.cs
│   └── MyApp.Core/
│       ├── MyApp.Core.csproj
│       └── ...
└── tests/
```

```
my-project/               ← launch forgiven from here
├── MyApp.csproj          ← single-project layout also works
└── Program.cs
```

---

## First-run startup time

`csharp-ls` loads and analyses the entire solution on startup. On a large
solution this can take **30–90 seconds** before navigation requests return
results. During this period:

- The status bar shows `"Finding definition…"` (or similar) after you trigger
  a request.
- Diagnostics (gutter markers) will start appearing gradually as files are
  analysed.

This is normal — wait for the initial indexing to complete. Subsequent
navigation requests in the same session are fast.

To monitor startup: open the diagnostics overlay with `SPC d d` — connected
LSP servers appear with a green indicator once initialised.

---

## Verifying the connection

1. Open a `.cs` file inside your project.
2. Press `SPC d d` — the diagnostics overlay should list `csharp` under
   **LSP Servers**.
3. Press `SPC l s` — after indexing completes, a symbol list appears showing
   the classes and methods in the current file.

If `csharp` does not appear in the diagnostics overlay:

- Confirm `forgiven` was launched from a directory containing a `.sln` or
  `.csproj` file.
- Confirm the `command` in `config.toml` is an absolute path (for the DLL
  approach) or that `csharp-ls` is on your `$PATH` (for the tool approach).
- Check `SPC d l` (log file) for startup errors from `csharp-ls`.

---

## Using the location list

When a request returns multiple results (e.g. an interface with several
implementations, or a symbol referenced in many files), a popup appears:

```
┌─ References (12) ──────────────────────────────────────┐
│   UserService.cs:45                                    │
│ ► UserController.cs:23                                 │
│   UserRepository.cs:88                                 │
│   ...                                                  │
│  j/k  navigate   Enter  jump   Esc  close              │
└────────────────────────────────────────────────────────┘
```

| Key | Action |
|-----|--------|
| `j` / `↓` | Move down |
| `k` / `↑` | Move up |
| `Enter` | Open file and jump to line |
| `Esc` or `q` | Close without navigating |

---

## Advanced: custom initialization options

`csharp-ls` accepts initialization options via LSP's `initialize` request. You
can pass these through `forgiven`'s config:

```toml
[[lsp.servers]]
language = "csharp"
command  = "dotnet"
args     = ["/path/to/CSharpLanguageServer.dll"]

[lsp.servers.initialization_options]
# Example: increase analysis timeout for large solutions
documentAnalysisTimeoutMs = 120000
```

---

## Known limitations

| Limitation | Status |
|-----------|--------|
| Hover tooltip (`SPC l h`) | Not yet implemented |
| Rename (`SPC l r`) | Not yet implemented |
| `Microsoft.CodeAnalysis.LanguageServer` (Roslyn) | Requires named-pipe transport, not yet supported |
| OmniSharp | Not recommended — known LSP protocol violations |
| Go to implementation (`SPC l i`) | Not yet implemented |

---

## Troubleshooting

**"No LSP client for 'csharp'"**
The server did not start or failed to initialise. Check `SPC d l` for errors.
Most common causes: wrong path in `args`, .NET SDK not on `$PATH`, or no
`.sln`/`.csproj` in the launch directory.

**"Finding definition…" hangs for minutes then nothing**
The server is still indexing. Wait for diagnostics to appear in the gutter —
that signals the server is ready.

**Diagnostics overlay shows `csharp` but navigation still fails**
Note that the diagnostics overlay lists *configured* servers, not necessarily
*connected* ones. A server that crashes immediately after start would still
appear in the list. Use `SPC d l` to see the actual log output.

**Symbols list is empty**
`csharp-ls` may return a `DocumentSymbol` response before it has fully parsed
the file. Try `SPC l s` again after a few seconds.
