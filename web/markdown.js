// Markdown for what Bots and users write: fenced code, headings, lists (nested, ordered,
// tasks), tables, quotes and rules as blocks; code, bold, italic, strikethrough and links in
// a line. Text is escaped before any markup is added, so a message never becomes HTML of
// its own.
'use strict'

const escape = text => String(text ?? '').replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]))

const MD_FENCE = /^\s*(`{3,}|~{3,})\s*([^`\s]*)/
const MD_HEADING = /^\s{0,3}(#{1,6})\s+(.*?)\s*#*\s*$/
const MD_RULE = /^\s{0,3}([-*_])(\s*\1){2,}\s*$/
const MD_QUOTE = /^\s{0,3}>\s?/
const MD_ITEM = /^(\s*)([-*+]|\d{1,9}[.)])\s+(.*)$/
const MD_TABLE_RULE = /^\s*\|?\s*:?-+:?\s*(\|\s*:?-+:?\s*)*\|?\s*$/

const indentOf = line => line.match(/^\s*/)[0].replace(/\t/g, '    ').length
const isTableStart = (lines, i) => lines[i]?.includes('|') && MD_TABLE_RULE.test(lines[i + 1] ?? '') && lines[i + 1].includes('-')
const startsBlock = (lines, i) => MD_FENCE.test(lines[i]) || MD_HEADING.test(lines[i]) || MD_RULE.test(lines[i]) || MD_QUOTE.test(lines[i]) || MD_ITEM.test(lines[i]) || isTableStart(lines, i)

// A web address ends before Chinese punctuation, and before closing punctuation it does not open.
function trimUrl(url) {
  let end = url.replace(/[.,;:!?'"]+$/, '')
  while (end.endsWith(')') && (end.match(/\(/g) ?? []).length < (end.match(/\)/g) ?? []).length) end = end.slice(0, -1)
  return end
}

function mdInline(raw) {
  const kept = []
  const keep = html => `\u0001${kept.push(html) - 1}\u0001`
  const link = (url, label) => `<a href="${escape(url)}" target="_blank" rel="noopener">${label}</a>`
  let text = String(raw ?? '').replace(/[\u0001\u0002]/g, '')
  // A backslash before punctuation shows the character itself, never as markup.
  const escaped = []
  text = text.replace(/\\([\\`*_{}\[\]()#+\-.!|~<>])/g, (_, c) => `\u0002${escaped.push(c) - 1}\u0002`)
  // Code first, so nothing inside it is read as markup.
  text = text.replace(/(`+)([^`\n]|[^`\n][\s\S]*?[^`\n])\1(?!`)/g, (_, ticks, code) => keep(`<code>${escape(code.trim() || code)}</code>`))
  // Images open as links (a message never loads remote pictures by itself); an optional "title" is dropped.
  text = text.replace(/(!?)\[([^\]\n]*)\]\(<?(https?:\/\/[^)\s>]+)>?(?:\s+(?:"[^"\n]*"|'[^'\n]*'))?\)/g, (_, image, label, url) => keep(link(url, label ? mdEmphasis(escape(label)) : escape(url))))
  text = text.replace(/<(https?:\/\/[^\s<>]+)>/g, (_, url) => keep(link(url, escape(url))))
  text = text.replace(/https?:\/\/[^\s<>"'`，。！？；：、（）【】「」《》]+/g, url => {
    const clean = trimUrl(url)
    return keep(link(clean, escape(clean))) + url.slice(clean.length)
  })
  // <br> is the usual way to break a line inside a table cell; it is the only tag kept.
  return mdEmphasis(escape(text)).replace(/&lt;br\s*\/?&gt;/gi, '<br>').replace(/\u0001(\d+)\u0001/g, (_, n) => kept[Number(n)]).replace(/\u0002(\d+)\u0002/g, (_, n) => escape(escaped[Number(n)]))
}

function mdEmphasis(html) {
  return html
    .replace(/\*\*\*(?=\S)([^\n]*?\S)\*\*\*/g, '<strong><em>$1</em></strong>')
    .replace(/\*\*(?=\S)((?:[^\n]|\n(?!\n))*?\S)\*\*/g, '<strong>$1</strong>')
    .replace(/(^|[^\w])__(?=\S)([^\n]*?\S)__(?!\w)/g, '$1<strong>$2</strong>')
    .replace(/~~(?=\S)((?:[^\n]|\n(?!\n))*?\S)~~/g, '<del>$1</del>')
    .replace(/(^|[^*\w\\])\*(?=[^\s*])([^*\n]*?[^\s*])?\*(?![*\w])/g, (m, before, inner) => (inner === undefined ? m : `${before}<em>${inner}</em>`))
    .replace(/(^|[^\w])_(?=[^\s_])([^_\n]*?[^\s_])_(?!\w)/g, '$1<em>$2</em>')
}

function mdCells(line) {
  const body = line.trim().replace(/^\|/, '').replace(/\|$/, '')
  const cells = []
  let cell = '', ticks = false
  for (let i = 0; i < body.length; i++) {
    const c = body[i]
    if (c === '\\' && body[i + 1] === '|') { cell += '|'; i++; continue }
    if (c === '`') ticks = !ticks
    if (c === '|' && !ticks) { cells.push(cell.trim()); cell = ''; continue }
    cell += c
  }
  cells.push(cell.trim())
  return cells
}

function mdTable(lines, i) {
  const head = mdCells(lines[i])
  const align = mdCells(lines[i + 1]).map(c => (c.startsWith(':') && c.endsWith(':') ? 'center' : c.endsWith(':') ? 'right' : ''))
  const cell = (tag, text, n) => `<${tag}${align[n] ? ` style="text-align:${align[n]}"` : ''}>${mdInline(text)}</${tag}>`
  const rows = []
  let j = i + 2
  while (j < lines.length && lines[j].trim() && lines[j].includes('|')) rows.push(mdCells(lines[j++]))
  const html = `<div class="md-table"><table><thead><tr>${head.map((c, n) => cell('th', c, n)).join('')}</tr></thead><tbody>${rows.map(r => `<tr>${head.map((_, n) => cell('td', r[n] ?? '', n)).join('')}</tr>`).join('')}</tbody></table></div>`
  return [html, j]
}

function mdList(lines, i) {
  const first = lines[i].match(MD_ITEM)
  const indent = indentOf(first[1])
  const ordered = /\d/.test(first[2])
  const items = []
  while (i < lines.length) {
    const line = lines[i]
    const m = line.match(MD_ITEM)
    if (m && indentOf(m[1]) <= indent && !MD_RULE.test(line)) {
      if (/\d/.test(m[2]) !== ordered) break
      items.push({ pad: m[0].length - m[3].length, lines: [m[3]] })
      i++
      continue
    }
    if (!line.trim()) {
      // A blank line keeps the list going only when more of it follows.
      let k = i + 1
      while (k < lines.length && !lines[k].trim()) k++
      const next = lines[k]
      const more = next !== undefined && (indentOf(next) > indent || (MD_ITEM.test(next) && indentOf(next.match(MD_ITEM)[1]) <= indent))
      if (!more) break
      items.at(-1).lines.push('')
      i++
      continue
    }
    if (indentOf(line) > indent) {
      items.at(-1).lines.push(line.replace(/\t/g, '    ').slice(Math.min(indentOf(line), items.at(-1).pad)))
      i++
      continue
    }
    // A plain line right under an item carries on its text.
    if (lines[i - 1]?.trim() && !startsBlock(lines, i)) {
      items.at(-1).lines.push(line.trim())
      i++
      continue
    }
    break
  }
  const start = ordered ? parseInt(first[2], 10) : 1
  const tag = ordered ? 'ol' : 'ul'
  const html = items.map(item => {
    let body = mdBlocks(item.lines)
    const tight = !item.lines.includes('') ? body.replace(/^<p>([\s\S]*?)<\/p>/, '$1') : body
    const task = tight.match(/^\[([ xX])\]\s+/)
    const box = task ? `<input type="checkbox" disabled${task[1] !== ' ' ? ' checked' : ''}> ` : ''
    body = task ? tight.slice(task[0].length) : tight
    return `<li${task ? ' class="task"' : ''}>${box}${body}</li>`
  }).join('')
  return [`<${tag}${ordered && start !== 1 ? ` start="${start}"` : ''}>${html}</${tag}>`, i]
}

function mdBlocks(lines) {
  const out = []
  let i = 0
  while (i < lines.length) {
    const line = lines[i]
    if (!line.trim()) { i++; continue }
    const fence = line.match(MD_FENCE)
    if (fence) {
      const close = new RegExp(`^\\s*${fence[1][0] === '`' ? '`' : '~'}{${fence[1].length},}\\s*$`)
      const code = []
      i++
      while (i < lines.length && !close.test(lines[i])) code.push(lines[i++])
      i++
      out.push(`<pre><code${fence[2] ? ` data-lang="${escape(fence[2])}"` : ''}>${escape(code.join('\n'))}</code></pre>`)
      continue
    }
    const heading = line.match(MD_HEADING)
    if (heading) {
      out.push(`<div class="md-h md-h${heading[1].length}">${mdInline(heading[2])}</div>`)
      i++
      continue
    }
    if (MD_RULE.test(line)) { out.push('<hr>'); i++; continue }
    if (MD_QUOTE.test(line)) {
      const quoted = []
      while (i < lines.length && lines[i].trim() && (MD_QUOTE.test(lines[i]) || !startsBlock(lines, i))) quoted.push(lines[i++].replace(MD_QUOTE, ''))
      out.push(`<blockquote>${mdBlocks(quoted)}</blockquote>`)
      continue
    }
    if (isTableStart(lines, i)) {
      const [html, next] = mdTable(lines, i)
      out.push(html)
      i = next
      continue
    }
    if (MD_ITEM.test(line)) {
      const [html, next] = mdList(lines, i)
      out.push(html)
      i = next
      continue
    }
    const para = [line.trim()]
    i++
    while (i < lines.length && lines[i].trim() && !startsBlock(lines, i) && !/^\s{0,3}=+\s*$/.test(lines[i])) para.push(lines[i++].trim())
    if (i < lines.length && /^\s{0,3}=+\s*$/.test(lines[i])) { i++; out.push(`<div class="md-h md-h1">${mdInline(para.join(' '))}</div>`); continue }
    out.push(`<p>${mdInline(para.join('\n')).replace(/\n/g, '<br>')}</p>`)
  }
  return out.join('')
}

function markdown(text) {
  return mdBlocks(String(text ?? '').replace(/\r\n?/g, '\n').split('\n'))
}

if (typeof module !== 'undefined') module.exports = { markdown, escape }
