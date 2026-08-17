---
name: count-files
description: Count files in a directory accurately — include hidden files (dotfiles) by default, exclude only . and .., distinguish files from directories
trigger: count, how many, number of files, count files, count directories, list files
---

When asked to count files (or directories) in a directory, be precise. Apply
these rules to every count:

## What counts as a file

- **Hidden files ARE real files.** Anything whose name begins with a `.`
  (e.g. `.gitignore`, `.env`, `.eslintrc`, `.bashrc`) is a regular file and
  **must be counted** unless the user explicitly asks to ignore hidden/dot
  files. Default = include them.
- `.` (current directory) and `..` (parent directory) are directory entries —
  hard links, not files. `ls -a` and `ls -la` list them. NEVER count them.
- A directory is not a file. "Entries" (everything `ls` lists) ≠ "files" ≠
  "directories". Use the exact word the user used.
- `.DS_Store` (macOS Finder metadata), `Thumbs.db` (Windows), and `*.swp`
  (vim) are editor/OS noise rather than project files, but they are still
  regular files — the default count includes them. Only call them out
  separately if the user asks specifically for "project files".

## Correct commands

Count **regular files** in the current directory — hidden files INCLUDED,
`.`/`..` and subdirectories excluded. **This is the default:**

```
find . -maxdepth 1 -type f | wc -l
```

Count regular files **excluding hidden ones** — use ONLY when the user
explicitly asks to ignore hidden/dot files:

```
find . -maxdepth 1 -type f ! -name '.*' | wc -l
```

Count **directories**:

```
find . -maxdepth 1 -type d | wc -l
```

Count **all entries** `ls` would show (rarely what the user wants):

```
ls -A | wc -l      # all entries except . and ..
ls -1 | wc -l      # non-hidden entries
```

## Method

1. Default to **including** hidden files. Only add `! -name '.*'` if the user
   explicitly says to ignore hidden/dot files.
2. Pick the command that matches the user's exact word ("files" → `-type f`).
3. Run it via `exec_shell` and read the integer from stdout.
4. State the answer in plain prose: "There are N regular files in the current
   directory (M of them hidden)." Break out the hidden count when relevant.
5. Never infer a count from `ls -la` output by eye — the `total N` header and
   the `.`/`..` lines make manual counting error-prone. Prefer `find … | wc -l`.
