# prompts: /review, /explain, /commit-msg and /fix-test send a prepared prompt; text after the command is added to it

Commands for you, not for the model (they cost no prompt bytes). Each shows its prompt and asks `send? [Y/n]`; Enter
sends it as your next message. Text after the command goes at the end, after a blank line:

- `/review [FOCUS]`: review the uncommitted changes for bugs, risks and missing tests.
- `/explain [WHAT]`: explain how a part of this project works, with the files and lines it rests on.
- `/commit-msg [HINT]`: write a commit message for the staged changes.
- `/fix-test [TEST]`: run the failing test, find the cause and fix it.

Each prompt is the file `cmd/NAME.md`. Your own: copy this module, add `cmd/NAME.md` and a link
`cmd/NAME -> ../.henki/template`, and add the copy with `henki mods add ./DIR`.
