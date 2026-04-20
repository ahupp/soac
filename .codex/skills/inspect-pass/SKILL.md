---
name: inspect-pass
description: Open the local web inspector at a specific tracked pass for a concrete source example. Use when the user asks to show a transform behavior in the inspector, compare passes visually, or jump directly to one named pass.
---

# Inspect a specific pass

Use this workflow when the user wants a concrete transform example in the web inspector.

## Workflow

1. Ensure the web inspector assets exist.
   - If needed, run `just build-web-inspector`.

2. Ensure the local inspector server is running.
   - Prefer `cargo run -p soac-inspector`.
   - Keep it in a long-lived exec session.
   - Reuse an existing server on `http://127.0.0.1:8000/` when it is already serving the inspector.

3. Open the inspector in a browser tab.
   - Use `http://127.0.0.1:8000/?src=...&pass=...`
   - `pass` selects the right-hand pane by tracked pass name.
   - `src` should be URL-encoded Python source for the concrete example.

4. Verify the right-hand title matches the requested pass.

5. In the response, state:
   - the exact pass you opened
   - the example source shape
   - the concrete thing to look for in that pass

## Notes

- The left pane shows the immediately previous step.
- If the requested pass is missing for that source, the page falls back to the nearest valid pass window.
