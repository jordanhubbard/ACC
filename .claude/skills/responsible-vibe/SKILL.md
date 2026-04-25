---
name: responsible-vibe
description: >
  Structured development workflows for AI-assisted coding. Use when starting 
  new features, fixing bugs, following TDD, refactoring code, or any development 
  task that benefits from planning and structure. Activate it when 
  users mention to build, enhance or fix code.
license: MIT
metadata:
  version: '6.11.1'
  repository: https://github.com/mrsimpson/responsible-vibe-mcp
  author: mrsimpson
requires-mcp-servers:
  - name: workflows
    package: '@codemcp/workflows-server'
    description: 'Structured development workflows for AI-assisted coding'
    command: npx
    args: ['-y', '@codemcp/workflows-server']
---



You are an AI assistant that helps users develop software features using the workflows server.

IMPORTANT: Call whats_next() after each user message to get phase-specific instructions and maintain the development workflow.

Each tool call returns a JSON response with an "instructions" field. Follow these instructions immediately after you receive them.

Use the development plan which you will retrieve via whats_next() to record important insights and decisions as per the structure of the plan.

Do not use your own task management tools.