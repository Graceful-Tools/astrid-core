// Runs astrid-web's markdown renderer over a case table and prints what it draws, as blocks.
// Invoked by ../export-from-web.mjs; not useful on its own.
//
// WHAT IS COMPARED. Web renders to HTML (`renderMarkdownWithLinks`, lib/markdown.ts — `marked`
// with GFM and `breaks: true`, Astrid's references swapped out first, then DOMPurify). The core
// renders the same text to a tree of blocks and inline runs (`markdown::render`), because a native
// shell draws paragraphs and runs, not markup. The contract is what a reader SEES, so the HTML is
// read back into the core's tree here, in the browser's own terms:
//
//   - The renderer runs in a jsdom window, so it takes its browser path: DOMPurify with the
//     rich-text allowlist, exactly what a page shows. (Without a window it takes a regex fallback
//     that no reader ever sees.)
//   - Blocks: p, h1–h6, pre>code (language from `language-*`), ul/ol (with `start`), li (a
//     checkbox `input` makes it a task item), blockquote, hr, table (alignment from the header
//     cells' `align`). Inline content outside any block — what DOMPurify leaves of a `<div>` —
//     is a paragraph, as it would flow on the page.
//   - Inlines: text with bold (strong), italic (em), strike (del), code and link; br is a line
//     break. Adjacent runs in the same style are one run, as the core writes them.
//   - An anchor whose href is /u/<id>, /lists/<id> or /?task=<id> is a reference pill: kind,
//     label (its text without the sigil) and the decoded id. Any other relative href is the app's
//     own page, and the core writes those absolute (https://astrid.cc/…), so they are read so.
//     An anchor DOMPurify stripped the href from is plain text.
//   - An anchor is a pill only when it is drawn as one (the pill classes), not merely when it
//     points where a pill would: `[home](/lists/x)` is an ordinary link.
//   - Renderer artefacts are not content and are dropped: the space marked prints after a task
//     checkbox, the newline that closes a raw-HTML block, and an empty <p> (all DOMPurify
//     leaves of an image).
//
// Divergences found this way are recorded in docs/CONTRACTS.md (D38) and their cases routed to
// `disputed` by ./disputed.mjs.
//
// Usage: node contracts/drivers/markdown.mjs <path-to-astrid-web>

import { join } from 'node:path'
import { createRequire } from 'node:module'
import { pathToFileURL } from 'node:url'
import { registerWebAliases } from './alias-loader.mjs'
import { partition } from './disputed.mjs'

const webRoot = process.argv[2]
if (!webRoot) {
  console.error('usage: node contracts/drivers/markdown.mjs <path-to-astrid-web>')
  process.exit(2)
}

// A browser window before anything imports DOMPurify, which binds to `window` at load.
const requireFromWeb = createRequire(join(webRoot, 'package.json'))
let JSDOM
try {
  ;({ JSDOM } = requireFromWeb('jsdom'))
} catch {
  console.error(`markdown driver: astrid-web's packages are not installed at ${webRoot} — run \`npm ci\` there (marked, DOMPurify and jsdom are what it runs)`)
  process.exit(2)
}
const dom = new JSDOM('<!doctype html><html><body></body></html>', { url: 'https://astrid.cc/' })
globalThis.window = dom.window
globalThis.document = dom.window.document

registerWebAliases(webRoot)

const { renderMarkdownWithLinks } = await import(pathToFileURL(join(webRoot, 'lib/markdown.ts')).href)

const APP_ORIGIN = 'https://astrid.cc'
const PILLS = [
  ['/u/', 'user', '@'],
  ['/lists/', 'list', '#'],
  ['/?task=', 'task', '!'],
]

// ── HTML → blocks ─────────────────────────────────────────────────────────────────────────

const BLOCK_TAGS = new Set(['P', 'H1', 'H2', 'H3', 'H4', 'H5', 'H6', 'PRE', 'UL', 'OL', 'BLOCKQUOTE', 'HR', 'TABLE'])

function blocksOf(container, { skipCheckbox = false } = {}) {
  const blocks = []
  let pending = []
  const flush = () => {
    const inlines = inlinesOf(pending, {})
    trimEdges(inlines)
    if (inlines.length > 0) blocks.push({ kind: 'paragraph', inlines })
    pending = []
  }
  for (const node of container.childNodes) {
    if (skipCheckbox && isCheckbox(node)) continue
    if (node.nodeType === 1 && BLOCK_TAGS.has(node.tagName)) {
      flush()
      const b = block(node)
      if (b) blocks.push(b)
    } else {
      pending.push(node)
    }
  }
  flush()
  return blocks
}

// Newlines at the edge of loose inline content are HTML source formatting, not text.
function trimEdges(inlines) {
  const isText = (i) => i && i.kind === 'text'
  while (isText(inlines[0]) && /^\s*$/.test(inlines[0].text)) inlines.shift()
  while (isText(inlines.at(-1)) && /^\s*$/.test(inlines.at(-1).text)) inlines.pop()
  if (isText(inlines[0])) inlines[0].text = inlines[0].text.replace(/^\n+/, '')
  if (isText(inlines.at(-1))) inlines.at(-1).text = inlines.at(-1).text.replace(/\n+$/, '')
}

function isCheckbox(node) {
  return node.nodeType === 1 && node.tagName === 'INPUT' && node.getAttribute('type') === 'checkbox'
}

function block(node) {
  switch (node.tagName) {
    case 'P': {
      const inlines = inlinesOf(node.childNodes, {})
      // An empty <p> — what DOMPurify leaves of an image — draws nothing.
      return inlines.length ? { kind: 'paragraph', inlines } : null
    }
    case 'H1': case 'H2': case 'H3': case 'H4': case 'H5': case 'H6':
      return { kind: 'heading', level: Number(node.tagName[1]), inlines: inlinesOf(node.childNodes, {}) }
    case 'PRE': {
      const code = node.querySelector('code')
      const language = (code?.className.match(/language-(\S+)/) || [])[1] ?? null
      return { kind: 'code', language, text: (code ?? node).textContent }
    }
    case 'UL':
    case 'OL':
      return {
        kind: 'list',
        ordered: node.tagName === 'OL',
        start: node.tagName === 'OL' && node.hasAttribute('start') ? Number(node.getAttribute('start')) : 1,
        items: [...node.children].filter((li) => li.tagName === 'LI').map(item),
      }
    case 'BLOCKQUOTE':
      return { kind: 'quote', blocks: blocksOf(node) }
    case 'HR':
      return { kind: 'rule' }
    case 'TABLE': {
      const headCells = [...node.querySelectorAll('thead th')]
      const rows = [...node.querySelectorAll('tbody tr')].map((tr) =>
        [...tr.children].map((td) => inlinesOf(td.childNodes, {})))
      return {
        kind: 'table',
        alignments: headCells.map((th) => th.getAttribute('align') ?? 'none'),
        header: headCells.map((th) => inlinesOf(th.childNodes, {})),
        rows,
      }
    }
  }
  throw new Error(`unhandled block ${node.tagName}`)
}

function item(li) {
  // The checkbox is the item's first child, or its first paragraph's in a loose list.
  const holder = isCheckbox(li.firstChild) ? li : li.firstElementChild?.tagName === 'P' && isCheckbox(li.firstElementChild.firstChild) ? li.firstElementChild : null
  let checked = null
  if (holder) {
    const box = holder.firstChild
    checked = box.hasAttribute('checked')
    // marked prints `<input …> ` — the space is the renderer's, not the item's text.
    const after = box.nextSibling
    if (after && after.nodeType === 3) after.textContent = after.textContent.replace(/^ /, '')
    box.remove()
  }
  return { checked, blocks: blocksOf(li) }
}

function inlinesOf(nodes, style) {
  const out = []
  for (const node of nodes) inline(node, style, out)
  return out
}

function push(out, text, style) {
  if (text === '') return
  const run = {
    kind: 'text',
    text,
    bold: !!style.bold,
    italic: !!style.italic,
    strike: !!style.strike,
    code: !!style.code,
    link: style.link ?? null,
  }
  const last = out.at(-1)
  if (last && last.kind === 'text' && ['bold', 'italic', 'strike', 'code', 'link'].every((k) => last[k] === run[k])) {
    last.text += text
  } else {
    out.push(run)
  }
}

function inline(node, style, out) {
  if (node.nodeType === 3) return push(out, node.textContent, style)
  if (node.nodeType !== 1) return
  switch (node.tagName) {
    case 'BR':
      out.push({ kind: 'lineBreak' })
      return
    case 'STRONG':
      return node.childNodes.forEach((c) => inline(c, { ...style, bold: true }, out))
    case 'EM':
      return node.childNodes.forEach((c) => inline(c, { ...style, italic: true }, out))
    case 'DEL':
      return node.childNodes.forEach((c) => inline(c, { ...style, strike: true }, out))
    case 'CODE':
      return node.childNodes.forEach((c) => inline(c, { ...style, code: true }, out))
    case 'A': {
      const href = node.getAttribute('href')
      // A pill is drawn as one, not merely linked to the same page: `[home](/lists/x)` is an
      // ordinary link to a list, styled as a link.
      const pill = href && /\brounded\b/.test(node.getAttribute('class') ?? '') &&
        PILLS.find(([prefix]) => href.startsWith(prefix))
      if (pill) {
        const [prefix, reference, sigil] = pill
        const text = node.textContent
        out.push({
          kind: 'reference',
          reference,
          label: text.startsWith(sigil) ? text.slice(sigil.length) : text,
          id: decodeURIComponent(href.slice(prefix.length)),
        })
        return
      }
      const link = href == null ? null : href.startsWith('/') && !href.startsWith('//') ? APP_ORIGIN + href : href
      return node.childNodes.forEach((c) => inline(c, { ...style, link }, out))
    }
    default:
      // span, and whatever DOMPurify kept the text of.
      return node.childNodes.forEach((c) => inline(c, style, out))
  }
}

function render(text, identifiers) {
  const html = renderMarkdownWithLinks(text, identifiers ? { identifiers } : undefined)
  const body = dom.window.document.createElement('body')
  body.innerHTML = html
  return { html, blocks: blocksOf(body) }
}

// ── Cases ─────────────────────────────────────────────────────────────────────────────────

const CASES = [
  // Text and breaks
  ['plain', 'Buy oat milk'],
  ['newline-is-a-break', 'first line\nsecond line\nthird'],
  ['blank-line-is-a-paragraph', 'one\n\ntwo'],
  ['trailing-spaces-break', 'a  \nb'],
  ['backslash-break', 'a\\\nb'],
  ['unicode', 'Café ☕ — naïve 日本語 🎉'],
  ['entities', 'Fish &amp; chips &lt;3 & 5 > 4'],
  ['escapes', '\\*not italic\\* and 1\\. not a list'],
  ['reported-description', '##title\n**bold**\n*italics*\n(link)[https://google.com]'],
  // Emphasis
  ['bold', 'a **bold** word'],
  ['bold-underscore', 'a __bold__ word'],
  ['italic', 'an *italic* word'],
  ['italic-underscore', 'an _italic_ word'],
  ['bold-italic', '***both*** at once'],
  ['nested-emphasis', '**bold with *italic* inside**'],
  ['intraword-underscore', 'snake_case_name stays'],
  ['strike-double', 'was ~~wrong~~ right'],
  ['strike-single', 'was ~wrong~ right'],
  ['unclosed-emphasis', 'a *dangling star'],
  // Code
  ['inline-code', 'run `npm test` now'],
  ['inline-code-with-stars', 'literally `**not bold**`'],
  ['inline-code-double-backtick', 'a `` code with ` tick `` here'],
  ['fenced-code', '```\nconst x = 1\n```'],
  ['fenced-code-language', '```ts\nconst x: number = 1\n```'],
  ['fenced-code-tilde', '~~~\nplain\n~~~'],
  ['fenced-code-two-lines', '```\nline one\nline two\n```'],
  ['fenced-code-trailing-blank', '```\ncode\n\n```'],
  ['fenced-code-two-trailing-blanks', '```\ncode\n\n\n```'],
  ['fenced-code-empty', '```\n```'],
  ['indented-code-trailing-blank', '    x = 1\n\n\nafter'],
  ['indented-code-one-line', '    x = 1'],
  ['indented-code', '    indented code\n    more'],
  // Headings
  ['headings', '# One\n## Two\n### Three\n#### Four\n##### Five\n###### Six'],
  ['heading-needs-space', '##title'],
  ['heading-then-body', '### Deeper\nbody'],
  ['setext-heading', 'Title\n====='],
  ['heading-with-emphasis', '## Plan for **today**'],
  // Lists
  ['bullets-dash', '- one\n- two\n- three'],
  ['bullets-star', '* one\n* two'],
  ['bullets-plus', '+ one\n+ two'],
  ['ordered', '1. one\n2. two'],
  ['ordered-start', '3. three\n4. four'],
  ['nested-list', '- outer\n  - inner\n- next'],
  ['loose-list', '- one\n\n- two'],
  ['task-list', '- [ ] todo\n- [x] done'],
  ['task-list-uppercase', '- [X] done'],
  ['list-after-paragraph', 'Groceries:\n- milk\n- eggs'],
  ['list-item-two-lines', '- first line\n  continues'],
  ['list-with-emphasis', '- **bold** item\n- `code` item'],
  // Quotes and rules
  ['quote', '> quoted'],
  ['quote-two-lines', '> line one\n> line two'],
  ['nested-quote', '> outer\n>> inner'],
  ['quote-with-list', '> - a\n> - b'],
  ['rule-dashes', 'above\n\n---\n\nbelow'],
  ['rule-stars', '***'],
  // Tables
  ['table', '| a | b |\n|---|---|\n| 1 | 2 |'],
  ['table-aligned', '| l | c | r |\n|:--|:-:|--:|\n| 1 | 2 | 3 |'],
  ['table-inline', '| name | note |\n|---|---|\n| **Jo** | `x` |'],
  // Links
  ['link', 'see [the docs](https://example.com/docs)'],
  ['link-mailto', '[write](mailto:a@example.com)'],
  ['link-javascript', '[click](javascript:alert(1))'],
  ['link-relative', '[home](/lists/abc)'],
  ['link-http-www-upgraded', '[site](http://www.example.com/a)'],
  ['link-unparseable-host', '[bad](https://exa]mple.com/x)'],
  ['link-port', '[local](http://localhost:3000/x)'],
  ['link-bold-text', '[**bold link**](https://example.com)'],
  ['angle-autolink', '<https://example.com/a>'],
  ['bare-https', 'go to https://example.com/path?q=1 now'],
  ['bare-http', 'http://example.com'],
  ['bare-www', 'visit www.example.com today'],
  ['bare-http-www-upgraded', 'http://www.example.com/x'],
  ['bare-trailing-period', 'See https://example.com.'],
  ['bare-trailing-comma', 'https://example.com, then'],
  ['bare-in-parens', '(https://example.com/a)'],
  ['bare-with-parens', 'https://en.wikipedia.org/wiki/Foo_(bar)'],
  ['bare-email', 'mail jon@example.com please'],
  ['bare-domain-only', 'example.com is not a link'],
  // References
  ['mention', 'Ask @[Jon Paris](user-1) about it'],
  ['list-reference', 'Filed in #[Groceries](list-1)'],
  ['task-reference', 'Blocked by ![Buy milk](task-1)'],
  ['three-references', '@[Ann](u1) #[Home](l1) ![Fix sink](t1)'],
  ['reference-in-bold', '**ask @[Ann](u1)**'],
  ['reference-in-code-span', '`@[Ann](u1)`'],
  ['reference-in-code-block', '```\n@[Ann](u1)\n```'],
  ['reference-label-markdown', '@[*Ann*](u1)'],
  ['reference-id-needs-encoding', '![Plan](a b/c)'],
  ['reference-then-link', '@[Ann](u1) [docs](https://example.com)'],
  ['reference-at-line-start', '@[Ann](u1)\nsecond line'],
  ['not-a-reference-empty-label', '@[](u1)'],
  ['email-like-reference', 'a@[b](c)'],
  ['image-syntax-empty', '![](https://example.com/x.png)'],
  // HTML
  ['html-inline-allowed', 'a <strong>strong</strong> word'],
  ['html-inline-disallowed', 'a <b>bold</b> word'],
  ['html-script', '<script>alert(1)</script>'],
  ['html-block-div', '<div>block text</div>'],
  ['html-img', 'x <img src="y" onerror="alert(1)"> z'],
  ['html-comment', 'a <!-- hidden --> b'],
  ['html-inline-script', 'a <script>alert(1)</script> b'],
  ['html-style-block', '<style>p { color: red }</style>\n\nafter'],
  // Empties
  ['empty', ''],
  ['whitespace-only', '   \n  '],
]

// Task identifiers (`AWTD-12`, `#12`) link only with a reader context (task 5f3453e2).
const CONTEXT = { projectKey: 'AWTD', keys: ['AWTD', 'OPS'], hidden: ['OPS-9'] }
const IDENTIFIER_CASES = [
  ['ids-with-context', 'Fixed by AWTD-12 and #4; see OPS-3', CONTEXT],
  ['ids-hidden-one', 'OPS-9 is hidden, OPS-8 is not', CONTEXT],
  ['ids-unknown-key', 'UTF-8 and COVID-19 stay prose', CONTEXT],
  ['ids-inside-task-pill', '![Follow up AWTD-12](t1)', CONTEXT],
  ['ids-in-code', '`AWTD-12`', CONTEXT],
  ['ids-without-context', 'AWTD-12 and #4', null],
]

const cases = [
  ...CASES.map(([id, text]) => ({ id, text, context: null, ...render(text, null) })),
  ...IDENTIFIER_CASES.map(([id, text, context]) => ({ id, text, context, ...render(text, context) })),
]

// D38: where marked-plus-DOMPurify and this crate's reading genuinely differ, and the core —
// which the Apple apps draw through (CoreRules.markdown) — keeps its own.
const D38 = new Set([
  'ordered-start', // web's sanitiser allowlist has no `start`, so every ordered list counts from 1
  'reference-in-code-span', // web draws a pill inside code; the core shows the reference as typed
  'reference-in-code-block',
  'html-inline-allowed', // web honours the inline tags its allowlist keeps; the core reads HTML as text
])
const DISPUTES = [
  {
    entry: 'D38',
    why: 'markdown: an ordered list\'s start number, a reference inside code, and inline HTML tags the web allowlists',
    applies: (c) => D38.has(c.id),
  },
]

const result = partition(cases, DISPUTES)

process.stdout.write(JSON.stringify({
  generatedFrom: 'lib/markdown.ts (renderMarkdownWithLinks, browser path), read back into blocks',
  ...result,
}))
