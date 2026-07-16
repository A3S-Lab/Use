# Presentation Workflows

Use stable one-based paths returned by `outline` or `get`, such as `/slide[1]`,
`/slide[1]/shape[1]`, and table row/cell paths.

## Inspect

```bash
a3s use office native get deck.pptx '/slide[1]' --depth 3 --json
a3s use office native query deck.pptx 'shape:contains("Roadmap")' --json
a3s use office native view deck.pptx outline --json
a3s use office native view deck.pptx issues --json
```

## Create and Edit

```bash
a3s use office native create deck.pptx --json
a3s use office native add deck.pptx / --type slide --text 'Roadmap' --json
a3s use office native add deck.pptx '/slide[1]' --type shape --text 'Q3' --json
a3s use office native add deck.pptx '/slide[1]' --type table --rows 3 --columns 4 --json
a3s use office native add deck.pptx '/slide[1]' --type picture --input diagram.png --alt 'System architecture' --json
a3s use office native set deck.pptx '/slide[1]/shape[1]' --text 'Updated roadmap' --json
a3s use office native swap deck.pptx '/slide[1]' '/slide[2]' --json
```

Basic table rows, cells, and virtual columns support bounded structural edits.
Merged cells, rich table styles, relationship-owning object copies, advanced
charts/media, animations, transitions, and theme fidelity remain incomplete.

## Verify and Preview

```bash
a3s use office native validate deck.pptx --json
a3s use office native view deck.pptx issues --type format --json
a3s use office native view deck.pptx svg --output deck.svg --json
a3s use office native view deck.pptx screenshot --output deck.png --json
```

SVG, HTML, and PNG output are deterministic semantic previews. They do not
prove PowerPoint layout, animation, morph, font, or theme fidelity.
