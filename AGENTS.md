# Agent guide

Instructions for coding agents working in this repository.

Zibaldone is a private, personal project. It has no license, so don't add a `LICENSE` file or
license headers.

## Environment

The development environment comes from [devenv](https://devenv.sh). Run commands inside it with
`devenv shell -- <command>`, or enter it with `devenv shell`. Entering the shell installs the git
hooks.

- `devenv.nix` defines the packages, environment variables, and git hooks.
- `devenv.yaml` pins the inputs, and `devenv.lock` records their versions.
- `.pre-commit-config.yaml` is a generated file. Don't edit or commit it.

## Tool configuration

Keep configuration for tooling in `.config/`, not in the repository root. Most tools don't look
there on their own, so point each one at its file from `devenv.nix`. Editors read `.editorconfig`
only from the root, so it stays there.

- `.config/gitlint`: commit message rules.
- `.config/markdownlint.yaml`: markdown structure rules.
- `.config/vale/vale.ini`: prose linting for markdown.
- `.config/vale/styles/config/vocabularies/Project/`: accepted and rejected terms.
- `.config/vale/styles/Google/`: the vendored Google style. Don't edit these files by hand. Run
  `vale sync` to update them. Sync replaces the whole folder, so the style's MIT license lives
  beside it in `.config/vale/styles/LICENSE-Google`.
- `.config/vale/styles/Project/`: project rules, including the general entries from the Google word
  lists and the multi-word entries from write-good's `TooWordy` rule.
- `.config/vale/styles/write-good/`: the vendored write-good style, which checks for weasel words,
  clichés, and weak sentence openings. Treat it like the Google style. Its MIT license lives in
  `.config/vale/styles/LICENSE-write-good`.
- `.config/yamlfmt.yaml`: formatting for `.yaml` files.
- `.config/yamllint.yaml`: linting for `.yaml` files.

Name YAML files with the `.yaml` extension, except under `.config/vale/`, where they must end in
`.yml` because Vale ignores rule files named any other way. The `yaml-extension` hook checks both.

## Checks

Every commit runs the git hooks in `devenv.nix`, and gitlint checks each commit message. Any failure
blocks the commit. Commit inside the devenv shell, with `devenv shell -- git commit`. To run the
hooks by hand:

```sh
devenv shell -- prek run --all-files
```

Some hooks fix files in place and then fail: trailing whitespace, final newlines, line endings,
formatting, and sorted files. Review the change, stage it, and commit again. Don't bypass the hooks
with `--no-verify`.

The hooks cover only the file types the repository has, so Rust, Kotlin, `.toml`, and GitHub Actions
workflows have none yet. When you add the first file of a new type, add hooks that format and lint
it in the same commit. The sibling `vibes` repository configures hooks for each of these, so copy
its setup.

## Line length

Wrap lines at 100 characters in every file: prose, source, comments, and configuration. Fill each
line close to the limit instead of breaking early.

Enforce the limit with a tool that fails the commit whenever possible. When you add a language or
file type, configure its formatter or linter for 100 characters. The `line-length` hook in
`devenv.nix` catches any text file that no other tool checks. If you have to exclude a file from
that hook, keep its lines within the limit by hand.

The hook allows no exceptions, so restructure content that's too long. In markdown, use
reference-style links for long URLs. In code, split long strings.

## Ordering

Sort lists alphabetically instead of grouping or categorizing them. This applies to imports,
dependency lists, package lists, configuration keys, attribute sets, and similar collections. Keep
another order only when the order changes behavior, and add a comment that explains why.

In Nix files, wrap each new sorted attribute set or list in `keep-sorted` markers so the hook
enforces the order. Use `block=yes` so the hook keeps each multi-line entry together:

```nix
{
  # keep-sorted start block=yes
  alpha = {
    enable = true;
  };
  beta = 1;
  # keep-sorted end
}
```

Markers can nest. If the entries have blank lines between them, add `newline_separated=yes` to the
start marker.

## Sorted files

A hook sorts the lines of `.gitignore` and the vocabulary files, ignoring case. Every line takes
part in the sort, so don't add comments or blank lines to these files.

## Writing markdown

Vale checks prose against the Google developer documentation style and parts of write-good, and
every enabled rule is an error. Before you commit, run `vale <file>` in the shell and fix each
alert.

- Write in active voice and present tense.
- Use sentence case for headings, with no closing period.
- Spell out acronyms on first use.
- Use contractions such as "don't" and "it's."
- Avoid Latin abbreviations such as "e.g." and "i.e."

For a project term that fails the spell check, add it to `accept.txt` in the vocabulary folder. Vale
then enforces the capitalization given there. To ban a term, add it to `reject.txt`. Keep the
vocabulary to terms the project's own files use.

Turn off a rule for a section only when the text can't comply, such as quoted material:

```markdown
<!-- vale Google.Passive = NO -->

Quoted text.

<!-- vale Google.Passive = YES -->
```

The markdownlint hook enforces structure: heading levels, blank lines around lists and code blocks,
and a language on each fenced code block. It skips the list marker rule, because `mdformat`
alternates markers to keep adjacent lists apart.

The `mdformat` hook fills each paragraph's lines up to 100 characters and overrides manual line
breaks.
