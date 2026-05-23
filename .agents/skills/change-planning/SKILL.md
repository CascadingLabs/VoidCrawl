---
name: change-planning
description: Use when planning a VoidCrawl change that may cross Rust core, PyO3, Python API, MCP, docs, or tests.
---
# VoidCrawl Change Planning
Identify:
- which layer owns the behavior
- whether the change crosses Rust/Python/MCP boundaries
- API/stub/docs impacts
- pool, async, GIL, or compatibility risks
- the smallest coherent implementation path

Return outcome, touched layers, non-goals, risks, and verification needs.
