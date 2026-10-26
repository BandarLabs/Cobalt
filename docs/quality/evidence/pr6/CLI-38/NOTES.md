# CLI-38: avoid required AI/chat, vague slogans and decorative dashboard clutter

Audited with greps and real runs, no code change needed:

- No AI/chat surface is required anywhere: case-insensitive greps for
  artificial-intelligence/chatbot/chatgpt/ai-powered phrasing over the CLI
  sources match nothing. The one agent-adjacent feature, Sidekick, is
  optional, off by default, and described in plain words.
- No slogans: the help header is the product name and "Usage:", the menu
  header is "Cobalt", and the body is factual verbs (check reads, preview
  shows, prepare writes, push sends, status reports).
- No decorative dashboard: a bare `kobo` in a pipe prints compact help and
  exits; in a terminal it prints a numbered menu of real actions. Nothing
  draws frames, banners, or status art.
