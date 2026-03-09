# ADR 0052: Drop C# / .NET LSP Support

**Date:** 2026-03-08
**Status:** Accepted

## Context

An attempt was made to support C# via the `csharp-ls` language server (`dotnet tool install -g csharp-ls`) as a replacement for OmniSharp, which has well-known LSP protocol violations (broken semantic tokens, inlay hint crashes, slow startup).

During evaluation the following blockers were encountered:

- `csharp-ls` 0.22.0 (latest) fails to install on .NET 9 SDK with `DotnetToolSettings.xml was not found in the package` — a packaging bug in the NuGet release.
- Even when an older version is installed, `csharp-ls` requires a `.csproj` or `.sln` at the workspace root for discovery, which is fragile in practice.
- The only fully correct .NET LSP — `Microsoft.CodeAnalysis.LanguageServer` (Roslyn LSP) — communicates over named pipes, not stdio, and is therefore incompatible with forgiven's current transport layer.
- OmniSharp has unfixed LSP non-compliance issues and is not a viable fallback.

The .NET LSP ecosystem is unstable and complex enough that offering unreliable C# support is worse than offering none.

## Decision

- Remove `csharp` from the default LSP server list in `src/lsp/config.rs`.
- Remove the `.cs` → `"csharp"` extension mapping from `src/lsp/mod.rs`.
- Remove the `~/.dotnet/tools` PATH augmentation added during this work.
- C# files will open without LSP services (syntax highlighting via syntect still works).

The `initialization_options` passthrough added to `LspServerConfig` is retained as it is useful for other servers.

## Consequences

- No broken C# LSP experience shipped.
- Users who need .NET LSP can configure a server manually via `config.toml` if they have one working in their environment.
- Revisit when named-pipe transport is implemented — at that point `Microsoft.CodeAnalysis.LanguageServer` becomes a viable first-class option.
