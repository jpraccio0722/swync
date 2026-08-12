## Development

When starting the dev server, use background mode:

```
astro dev --background
```

Manage the background server with `astro dev stop`, `astro dev status`, and `astro dev logs`.

## The function reference

`/docs/` is built from MDX in `src/content/docs/`. A file at the top of that
directory is a category (`signals.mdx`); a file inside the directory of the same
name is one of its builtins (`signals/saw.mdx`).

Edit the **body** freely — it is prose, and MDX, so a ```swync fence in it is
coloured by the same tokenizer the tutorial and the editor use.

Do not hand-edit the **frontmatter**. Parameters, arities, `receives` and
`returns` are decided in `swync-app/src-tauri/src/lang.rs`, and

```
npm run reference
```

dumps them to `src/data/metadata.json` and rewrites every page's frontmatter
from it, leaving the bodies alone. That is also what seeds a page for a UGen
newly added to `UGENS`, and what warns about a page whose name has left the
tables. `metadata.json` stays generated: nothing edits it by hand, and
`src/lib/highlight.ts` still reads the keywords and durations out of it.

## Documentation

Full documentation: https://docs.astro.build

Consult these guides before working on related tasks:

- [Adding pages, dynamic routes, or middleware](https://docs.astro.build/en/guides/routing/)
- [Working with Astro components](https://docs.astro.build/en/basics/astro-components/)
- [Using React, Vue, Svelte, or other framework components](https://docs.astro.build/en/guides/framework-components/)
- [Adding or managing content](https://docs.astro.build/en/guides/content-collections/)
- [Adding styles or using Tailwind](https://docs.astro.build/en/guides/styling/)
- [Supporting multiple languages](https://docs.astro.build/en/guides/internationalization/)
