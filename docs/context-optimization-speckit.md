This plan outlines the architecture for a Context-Optimized AI IDE using Rust and Tauri. It specifically targets the "SpecKit Tax"—the high token cost associated with Spec-Driven Development—by implementing Dynamic Context Slicing and Active Memory Management.
You can feed the markdown below directly to your LLM.
------------------------------
## Project Plan: "Focus" – The Spec-Optimized Rust IDE## 1. Executive Summary
Objective: Build a lightweight, AI-native IDE in Rust (Tauri) that supports Spec-Driven Development (SpecKit) while reducing token consumption by 40-60%.
Problem: Standard implementations of SpecKit blindly inject entire spec.md and plan.md files into the context window. Combined with chat history re-sends, this leads to exponential token decay.
Solution: Implement an intelligent Context Engine in Rust that "slices" specifications and "prunes" chat history before they reach the LLM, ensuring high relevance with low overhead.
------------------------------
## 2. Core Architecture (Rust + Tauri)## 2.1 The Backend: Context Engine (Rust)
Instead of a simple file reader, the Rust backend will act as a Context Gateway.

* Crate: notify for file watching.
* Crate: tree-sitter for parsing code structure (not just text).
* Crate: tiktoken-rs for real-time token counting.

Key Modules:

   1. ContextManager: Intercepts every user prompt. It checks the current token budget and decides what files to exclude.
   2. SpecSlicer: A regex/markdown parser that identifies the active task in tasks.md and extracts only the relevant requirement from spec.md.
   3. MemoryJanitor: A background thread that monitors chat length. When a session exceeds N tokens, it triggers a "Silent Summarization" (see Section 3).

## 2.2 The Frontend: Intent UI (Tauri/React)

* Spec Navigator: A sidebar that visualizes spec.md not as a text file, but as a tree of requirements.
* Token Fuel Gauge: A real-time visualizer showing exactly where tokens are going (e.g., "30% Spec", "50% History", "20% Code").

------------------------------
## 3. Strategy: Dynamic Context Slicing
Research indicates that LLMs suffer from "Lost in the Middle" phenomena when context exceeds 20k tokens. Slicing improves both cost and accuracy.
## 3.1 The "Spotlight" Protocol
When the user selects a task (e.g., "Implement User Auth"), the IDE does NOT send the whole spec.md.

   1. Parse: Rust parses spec.md into sections (Header, Auth, DB, UI).
   2. Filter: It identifies that "User Auth" depends only on the [Auth] and [Database] sections.
   3. Inject: It constructs a Virtual Context:
   
   # Virtual Spec (Synthesized)- Requirement: Users must log in via OAuth2 (extracted from line 50-55 of spec.md)- Schema: User table structure (extracted from plan.md)- Current Task: Implement JWT handler.
   
   Result: Reduces context from 20,000 tokens → ~1,500 tokens.

## 3.2 The "Archive" Pattern for tasks.md
As tasks are marked [x], the ContextManager automatically moves them to a hidden archive.md file or simply comments them out in the prompt payload. The LLM never sees completed work unless explicitly asked.
------------------------------
## 4. Session Memory: The "Rolling Summary"
Standard Chat Re-send creates 33% of your token waste. We will implement "Observation Masking" (JetBrains Research).
## 4.1 Algorithm: The 4-Turn Reset
The IDE maintains a "Sliding Window" of the last 4 exchanges.

   1. Trigger: After 4 user/AI turns.
   2. Action: The Rust backend spawns a cheap "Summarizer Agent" (using a smaller model like GPT-4o-mini or Haiku).
   3. Prompt: "Summarize the technical decisions made in the last 4 turns into a single bulleted list. Discard chit-chat."
   4. Update: The Chat History is wiped and replaced with:
   
   System: Previous context summary: [Summary]
   User: [Newest Message]
   
   
## 4.2 Ephemeral vs. Persistent State

* Ephemeral: "Can you fix this typo?" (Deleted after resolution).
* Persistent: "Let's use Postgres instead of SQLite." (Saved to constitution.md or plan.md immediately).

------------------------------
## 5. Implementation Roadmap## Phase 1: The Token Awareness System

* Task: Implement tiktoken-rs to display a live token count in the IDE footer.
* Task: Create a "Cost Breakdown" view that categorizes tokens into System Prompt, Spec Files, Chat History, and New Code.
* Goal: Visual proof of where the "leak" is.

## Phase 2: The Spec Slicer

* Task: Create a Rust module SpecParser that reads tasks.md.
* Task: Implement get_active_context():
* Input: Current cursor position or selected task.
   * Output: String containing only the active task and its parent spec section.
* Task: Wire this into the Chat Request payload, replacing the raw file dump.

## Phase 3: The Auto-Janitor

* Task: Implement MemoryJanitor struct.
* Task: Add a "Summarize & Clear" button to the UI (Manual trigger first).
* Task: Automate trigger when context > 10k tokens.

------------------------------
## 6. Research & References

* "Efficient Context Management" (JetBrains Research, 2025): Demonstrated that "Observation Masking" (hiding intermediate steps) reduces tokens by 40% without accuracy loss.
* "Context Engineering" (Anthropic): Recommends placing the most critical instructions (Spec) at the end of the prompt for better adherence (Recency Bias), allowing you to aggressively prune the middle.

## 7. Immediate Action Items (To stop the bleeding now)
While building the IDE, adopt this manual workflow:

   1. Atomic Specs: Break spec.md into spec_auth.md, spec_ui.md, etc. Only @mention the specific file you need.
   2. The "Constitution" Check: Remove constitution.md from the chat context. It should be a system prompt, not a file read.
   3. Aggressive Restarting: Restart the chat session every time you complete a checkbox in tasks.md.


