// Local, bounded attachment previews. File content is never executed in the app.
'use strict'

const FilePreview = (() => {
  const TEXT_LIMIT = 1024 * 1024, ZIP_LIMIT = 25 * 1024 * 1024
  const INLINE_LIMIT = 120 * 1024, XML_LIMIT = 2 * 1024 * 1024, IMAGE_LIMIT = 15 * 1024 * 1024, IMAGES_LIMIT = 60 * 1024 * 1024
  const textExtensions = new Set('txt text log out err conf config cfg ini env properties yaml yml toml xml xsd xsl rss atom json jsonc jsonl ndjson csv tsv sql sh bash zsh fish ps1 bat cmd py pyw js mjs cjs jsx ts tsx vue svelte html htm css scss sass less c h cc cpp hpp cs java kt kts go rs rb php swift scala r lua pl rkt ex exs erl hs dart proto graphql gql ipynb tex bib rst adoc diff patch dockerfile makefile gitignore gitattributes editorconfig'.split(' '))
  const officeExtensions = new Set(['docx', 'xlsx', 'pptx', 'odt', 'ods', 'odp'])
  let active, vendorPromise
  const inlineCache = new Map()
  const ext = item => String(item.name || '').toLowerCase().split('.').pop()
  function kind(item) {
    const suffix = ext(item), mime = String(item.mime || '').toLowerCase().split(';')[0]
    if (['md', 'markdown', 'mdown', 'mkd'].includes(suffix) || mime === 'text/markdown') return 'markdown'
    if (officeExtensions.has(suffix)) return suffix
    if (suffix === 'zip' || mime === 'application/zip') return 'zip'
    if (suffix === 'svg' || mime === 'image/svg+xml') return 'svg'
    if (['html', 'htm'].includes(suffix) || mime === 'text/html') return 'html'
    if (['csv', 'tsv'].includes(suffix) || ['text/csv', 'text/tab-separated-values'].includes(mime)) return suffix === 'tsv' || mime === 'text/tab-separated-values' ? 'tsv' : 'csv'
    if (suffix === 'json' || mime === 'application/json') return 'json'
    if (mime === 'application/pdf' || suffix === 'pdf') return 'pdf'
    if (mime.startsWith('image/')) return 'image'
    if (mime.startsWith('video/')) return 'video'
    if (mime.startsWith('audio/')) return 'audio'
    if (mime.startsWith('text/') || /application\/(?:.*\+xml|xml|.*json|javascript|x-sh|toml|yaml|x-yaml)/.test(mime) || textExtensions.has(suffix)) return 'text'
    return 'unknown'
  }
  function url(item) {
    // Previews can fetch only the app's stored media, never arbitrary URLs.
    const src = String(item.src || '')
    if (!/^media\/[^/\\\u0000-\u001f]+$/.test(src) || ['.', '..'].includes(src.slice(6))) throw new Error(t('Invalid file reference.'))
    return `/media/${encodeURIComponent(src.slice(6))}`
  }
  const info = item => JSON.stringify({ src: item.src, name: item.name, mime: item.mime, size: item.size })
  function button(item, label = 'Preview', cls = 'file-preview-button') {
    return `<button type="button" class="${cls}" data-file-preview="${escape(info(item))}">${escape(t(label))}</button>`
  }
  function inlineHtml(item) {
    const type = kind(item)
    if (!['markdown', 'json', 'text', 'csv', 'tsv'].includes(type) || item.size > INLINE_LIMIT) return ''
    return `<div class="file-inline bubble-plain" data-keep="file:${escape(item.id || item.src)}" data-file-inline="${escape(info(item))}" aria-busy="true"><span class="muted">${t('Loading…')}</span></div>`
  }
  async function bytes(item, limit, signal, partial = false) {
    const response = await fetch(url(item), { credentials: 'same-origin', signal, headers: partial ? { Range: `bytes=0-${limit - 1}` } : {} })
    if (!response.ok) throw new Error(t('File preview could not load.'))
    const reader = response.body.getReader(), chunks = []
    let size = 0, truncated = Number(item.size) > limit
    const range = response.headers.get('content-range')?.match(/\/(\d+)$/)
    truncated ||= range ? Number(range[1]) > limit : Number(response.headers.get('content-length')) > limit
    try {
      for (;;) {
        const { done, value } = await reader.read()
        if (done) break
        if (size + value.length > limit) {
          if (!partial) throw new Error(t('This file is too large to preview.'))
          chunks.push(value.subarray(0, limit - size)); size = limit; truncated = true; break
        }
        chunks.push(value); size += value.length
      }
    } finally { await reader.cancel().catch(() => {}); reader.releaseLock() }
    const result = new Uint8Array(size)
    let at = 0
    for (const chunk of chunks) { result.set(chunk, at); at += chunk.length }
    return { data: result, truncated }
  }
  function decode(data, truncated = false) {
    let text
    if (data[0] === 0xff && data[1] === 0xfe) text = new TextDecoder('utf-16le').decode(data)
    else if (data[0] === 0xfe && data[1] === 0xff) text = new TextDecoder('utf-16be').decode(data)
    else {
      try { text = new TextDecoder('utf-8', { fatal: true }).decode(data, { stream: truncated }) }
      catch { text = new TextDecoder('gb18030').decode(data) }
    }
    if (text.includes('\0')) throw new Error(t('This binary file has no text preview.'))
    return text.replace(/^\uFEFF/, '')
  }
  function csv(text, separator) {
    const rows = [], row = []
    let cell = '', quoted = false, limited = false
    for (let i = 0; i < text.length; i++) {
      const c = text[i]
      if (quoted) {
        if (c === '"' && text[i + 1] === '"') { cell += '"'; i++ }
        else if (c === '"') quoted = false
        else cell += c
      } else if (c === '"' && !cell) quoted = true
      else if (c === separator) { row.push(cell); cell = ''; if (row.length >= 100) { rows.push([...row]); limited = true; break } }
      else if (c === '\n' || c === '\r') {
        row.push(cell); rows.push([...row]); row.length = 0; cell = ''
        if (c === '\r' && text[i + 1] === '\n') i++
        if (rows.length >= 500) { limited = i < text.length - 1; break }
      } else cell += c
    }
    if (!limited && (cell || row.length)) { row.push(cell); rows.push([...row]) }
    return { rows, limited }
  }
  const table = (rows, header = false) => `<div class="preview-table-wrap"><table class="preview-table">${rows.map((row, i) => `<tr>${row.map(value => `<${header && i === 0 ? 'th' : 'td'}>${escape(value)}</${header && i === 0 ? 'th' : 'td'}>`).join('')}</tr>`).join('')}</table></div>`
  const sourceHtml = text => `<pre class="preview-source"><code>${escape(text)}</code></pre>`
  const notice = text => `<p class="preview-notice">${escape(t(text))}</p>`
  function renderText(text, type, truncated = false) {
    let html
    if (type === 'markdown') html = `<article class="preview-markdown bubble-plain">${markdown(text)}</article>`
    else if (['csv', 'tsv'].includes(type)) {
      const parsed = csv(text, type === 'tsv' ? '\t' : ',')
      html = table(parsed.rows, true) + (parsed.limited ? notice('Showing up to 500 rows and 100 columns.') : '')
    } else if (type === 'json' && !truncated) {
      try { html = sourceHtml(JSON.stringify(JSON.parse(text), null, 2)) }
      catch { html = notice('Invalid JSON; showing the original text.') + sourceHtml(text) }
    } else html = sourceHtml(text)
    return (truncated ? notice('Preview truncated. Download the file to read all content.') : '') + html
  }
  async function fillInline(root) {
    await Promise.all([...root.querySelectorAll('[data-file-inline]')].map(async element => {
      const item = JSON.parse(element.dataset.fileInline)
      delete element.dataset.fileInline
      const controller = new AbortController()
      // A detached message must not keep a pending preview request alive.
      const observer = new MutationObserver(() => { if (!element.isConnected) { controller.abort(); observer.disconnect() } })
      observer.observe(document.querySelector('#messages'), { childList: true, subtree: true })
      try {
        let result = inlineCache.get(item.src)
        if (!result) {
          const raw = await bytes(item, INLINE_LIMIT, controller.signal, true)
          result = { text: decode(raw.data, raw.truncated), truncated: raw.truncated }
          inlineCache.set(item.src, result)
          if (inlineCache.size > 16) inlineCache.delete(inlineCache.keys().next().value)
        }
        if (element.isConnected) element.innerHTML = renderText(result.text, kind(item), result.truncated)
      } catch (error) {
        if (error.name !== 'AbortError' && element.isConnected) element.innerHTML = `<span class="muted">${escape(error.message)}</span>${button(item, 'Retry preview')}`
      } finally { observer.disconnect(); element.setAttribute('aria-busy', 'false') }
    }))
  }
  function vendor() {
    if (globalThis.fflate) return Promise.resolve(globalThis.fflate)
    vendorPromise ??= new Promise((resolve, reject) => {
      const script = document.createElement('script')
      script.src = '/assets/vendor/fflate-0.8.2.js'
      script.onload = () => resolve(globalThis.fflate)
      script.onerror = () => { vendorPromise = null; script.remove(); reject(new Error(t('File preview could not load.'))) }
      document.head.append(script)
    })
    return vendorPromise
  }
  function xml(data) {
    const doc = new DOMParser().parseFromString(new TextDecoder().decode(data), 'application/xml')
    if (doc.querySelector('parsererror') || doc.doctype) throw new Error(t('This document is damaged or unsupported.'))
    return doc
  }
  const nodes = (node, name) => [...node.getElementsByTagNameNS('*', name)]
  const children = node => [...node.children]
  const local = node => node.localName
  const allText = node => nodes(node, 't').map(t => t.textContent).join('')
  function office(files, type) {
    const doc = path => { if (!files[path]) throw new Error(t('This document is damaged or unsupported.')); return xml(files[path]) }
    // Word and PowerPoint keep their layout: office-preview.js draws them from their own XML.
    if (type === 'docx') return OfficePreview.docx(files)
    if (type === 'pptx') return OfficePreview.pptx(files)
    if (type === 'xlsx') {
      const shared = files['xl/sharedStrings.xml'] ? nodes(doc('xl/sharedStrings.xml'), 'si').map(allText) : []
      const relationships = nodes(doc('xl/_rels/workbook.xml.rels'), 'Relationship')
      const sections = nodes(doc('xl/workbook.xml'), 'sheet').slice(0, 40).map(sheet => {
        const rel = relationships.find(r => r.getAttribute('Id') === sheet.getAttribute('r:id') && r.getAttribute('TargetMode') !== 'External')
        const target = rel?.getAttribute('Target') || ''
        const content = doc(target.startsWith('/xl/') ? target.slice(1) : `xl/${target}`)
        const rows = nodes(content, 'row').slice(0, 500).map(row => {
          const cells = []
          for (const cell of children(row).filter(n => local(n) === 'c').slice(0, 100)) {
            const letters = cell.getAttribute('r')?.match(/^[A-Z]+/)?.[0]
            const index = letters ? [...letters].reduce((n, c) => n * 26 + c.charCodeAt(0) - 64, 0) - 1 : cells.length
            if (index >= 100) continue
            while (cells.length < index) cells.push('')
            const value = nodes(cell, 'v')[0]?.textContent || ''
            const cellType = cell.getAttribute('t')
            cells[index] = cellType === 's' ? shared[Number(value)] || '' : cellType === 'inlineStr' ? allText(cell) : cellType === 'b' ? value === '1' ? 'TRUE' : 'FALSE' : value
          }
          return cells
        })
        return { name: sheet.getAttribute('name') || t('Worksheet'), html: table(rows) }
      })
      return { sections }
    }
    const content = doc('content.xml')
    const odfText = node => node.textContent || ''
    const odfRows = node => {
      const rows = []
      for (const row of nodes(node, 'table-row').slice(0, 500)) {
        const cells = []
        for (const cell of children(row).filter(n => ['table-cell', 'covered-table-cell'].includes(local(n)))) {
          const repeated = Math.min(Number(cell.getAttribute('table:number-columns-repeated')) || 1, 100 - cells.length)
          for (let i = 0; i < repeated; i++) cells.push(odfText(cell))
          if (cells.length >= 100) break
        }
        const repeat = Math.min(Number(row.getAttribute('table:number-rows-repeated')) || 1, 500 - rows.length)
        for (let i = 0; i < repeat; i++) rows.push(cells)
        if (rows.length >= 500) break
      }
      return rows
    }
    if (type === 'ods') return { sections: nodes(content, 'table').slice(0, 40).map(sheet => ({ name: sheet.getAttribute('table:name') || t('Worksheet'), html: table(odfRows(sheet)) })) }
    if (type === 'odp') return { sections: nodes(content, 'page').slice(0, 100).map((slide, i) => ({ name: slide.getAttribute('draw:name') || t('Slide {n}', { n: i + 1 }), html: nodes(slide, 'p').map(p => `<p>${escape(odfText(p))}</p>`).join('') })) }
    const text = nodes(content, 'text')[0]
    if (!text) throw new Error(t('This document is damaged or unsupported.'))
    return { sections: [{ name: t('Document'), html: children(text).map(n => local(n) === 'table' ? table(odfRows(n)) : `<${local(n) === 'h' ? 'h2' : 'p'}>${escape(odfText(n))}</${local(n) === 'h' ? 'h2' : 'p'}>`).join('') }] }
  }
  async function archive(data, type, signal) {
    const lib = await vendor(), entries = []
    signal.throwIfAborted()
    let expanded = 0, pictures = 0
    const wanted = name => type === 'docx' ? /^word\/(document\.xml|styles\.xml|numbering\.xml|_rels\/document\.xml\.rels)$/.test(name)
      : type === 'pptx' ? /^ppt\/(presentation\.xml|_rels\/presentation\.xml\.rels|(slides|slideLayouts|slideMasters)\/(_rels\/)?[^/]+\.xml(\.rels)?|theme\/[^/]+\.xml)$/.test(name)
        : type === 'xlsx' ? /^xl\/(workbook\.xml|sharedStrings\.xml|_rels\/workbook\.xml\.rels|worksheets\/sheet\d+\.xml)$/.test(name)
          : name === 'content.xml'
    const files = lib.unzipSync(data, { filter(entry) {
      if (entries.length >= 2000) throw new Error(t('This archive contains too many files.'))
      entries.push({ name: entry.name, size: entry.originalSize })
      if (type === 'zip') return false
      // Pictures of a document are optional: one too large is left out, not an error.
      if (['docx', 'pptx'].includes(type) && /^(word|ppt)\/media\/[^/]+\.(png|jpe?g|jpe|gif|bmp|webp|svg)$/i.test(entry.name)) {
        if (entry.originalSize > IMAGE_LIMIT || pictures + entry.originalSize > IMAGES_LIMIT) return false
        pictures += entry.originalSize
        return true
      }
      if (!wanted(entry.name)) return false
      expanded += entry.originalSize
      if (entry.originalSize > XML_LIMIT || expanded > 8 * XML_LIMIT) throw new Error(t('This document is too complex to preview.'))
      return true
    } })
    if (type === 'zip') return { sections: [{ name: t('Archive contents'), html: table([[t('File name'), t('File size')], ...entries.map(e => [e.name, `${e.size.toLocaleString()} B`])], true) }] }
    const result = office(files, type)
    if (!result.sections.length) throw new Error(t('This document is damaged or unsupported.'))
    return result
  }
  function safeDocument(text, type) {
    // No script, same-origin access, forms, navigation or remote resources.
    const doc = new DOMParser().parseFromString(type === 'svg' ? `<body>${text}</body>` : text, 'text/html')
    for (const el of doc.querySelectorAll('script,iframe,object,embed,base,meta,link,form,foreignObject')) el.remove()
    for (const el of doc.querySelectorAll('*')) for (const attr of [...el.attributes]) {
      if (/^on/i.test(attr.name) || ['href', 'xlink:href', 'action', 'formaction', 'srcdoc'].includes(attr.name.toLowerCase())) el.removeAttribute(attr.name)
    }
    const policy = "default-src 'none'; style-src 'unsafe-inline'; img-src data:; font-src data:; form-action 'none'; base-uri 'none'"
    return `<!doctype html><html><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="${policy}"><style>body{font-family:system-ui,sans-serif;padding:18px;overflow-wrap:anywhere}img,svg{max-width:100%;height:auto}table{border-collapse:collapse}td,th{padding:6px;border:1px solid #ddd}</style>${doc.head.innerHTML}</head><body>${doc.body.innerHTML}</body></html>`
  }
  function draw(state) {
    if (active !== state) return
    const body = document.querySelector('#file-preview-body'), tabs = document.querySelector('#file-preview-tabs')
    tabs.innerHTML = state.text !== undefined && ['markdown', 'html', 'svg', 'csv', 'tsv', 'json'].includes(state.type) ? `<div class="seg"><button type="button" data-preview-mode="rendered" class="${state.mode === 'rendered' ? 'on' : ''}" aria-pressed="${state.mode === 'rendered'}">${t('Preview')}</button><button type="button" data-preview-mode="source" class="${state.mode === 'source' ? 'on' : ''}" aria-pressed="${state.mode === 'source'}">${t('Source code')}</button></div>` : ''
    if (state.sections) {
      if (!state.page) tabs.innerHTML = `<label class="preview-section-label">${t('Section')}<select id="file-preview-section" aria-label="${t('Section')}">${state.sections.map((s, i) => `<option value="${i}" ${i === state.section ? 'selected' : ''}>${escape(s.name)}</option>`).join('')}</select></label>`
      if (state.deck) {
        // Every slide in one scroll; the menu jumps to a slide and follows the scrolling.
        body.innerHTML = notice('Drawn in the browser; fonts, charts and effects may differ slightly from Office.') + `<div class="pptx-deck">${state.sections.map((s, i) => `<figure class="pptx-frame" data-slide="${i}"><figcaption>${escape(s.name)}</figcaption>${s.html}</figure>`).join('')}</div>`
        const select = tabs.querySelector('select'), seen = new IntersectionObserver(entries => {
          const top = entries.filter(e => e.isIntersecting).sort((a, b) => a.boundingClientRect.top - b.boundingClientRect.top)[0]
          if (top && active === state) select.value = state.section = Number(top.target.dataset.slide)
        }, { root: body, threshold: 0.5 })
        body.querySelectorAll('.pptx-frame').forEach(frame => seen.observe(frame))
      } else if (state.page) body.innerHTML = notice('Drawn in the browser; fonts, charts and effects may differ slightly from Office.') + state.sections[0].html
      else body.innerHTML = (state.type === 'zip' ? '' : notice('Content preview only; complex layouts, images and charts may differ.')) + (['xlsx', 'ods'].includes(state.type) ? notice('Cells show saved values. Formulas are not recalculated; date formatting may differ.') : '') + `<article class="preview-markdown bubble-plain">${state.sections[state.section].html}</article>`
    } else if (state.text !== undefined) {
      if (state.mode === 'source') body.innerHTML = (state.truncated ? notice('Preview truncated. Download the file to read all content.') : '') + sourceHtml(state.text)
      else if (['html', 'svg'].includes(state.type)) {
        body.innerHTML = notice('Scripts and external resources are disabled in this preview.') + (state.truncated ? notice('Preview truncated. Download the file to read all content.') : '') + `<iframe class="preview-document-frame" sandbox="" referrerpolicy="no-referrer" title="${escape(state.item.name)}"></iframe>`
        body.querySelector('iframe').srcdoc = safeDocument(state.text, state.type)
      } else body.innerHTML = renderText(state.text, state.type, state.truncated)
    }
    body.setAttribute('aria-busy', 'false')
  }
  async function open(item, trigger) {
    close(false)
    let fileUrl
    try { fileUrl = url(item) } catch (error) { toast(error.message); return }
    const state = { item, type: kind(item), controller: new AbortController(), trigger, mode: 'rendered', section: 0 }
    active = state
    const dialog = document.querySelector('#file-preview-dialog')
    dialog.hidden = false
    dialog.innerHTML = `<div class="file-preview-window"><header class="file-preview-head"><div><h2 id="file-preview-title">${escape(item.name || t('File preview'))}</h2><small>${escape(t('File preview'))}</small></div><div class="file-preview-actions"><a class="button ghost" href="${escape(fileUrl)}" download="${escape(item.name)}">${t('Download')}</a><button type="button" class="ghost" data-preview-close>${t('Close')}</button></div></header><div id="file-preview-tabs"></div><div id="file-preview-body" aria-busy="true"><div class="preview-loading" role="status"><span class="skeleton-block"></span><span class="skeleton-block"></span><span class="skeleton-block"></span><p>${t('Loading…')}</p></div></div></div>`
    dialog.querySelector('[data-preview-close]').focus()
    const body = dialog.querySelector('#file-preview-body')
    try {
      if (['image', 'video', 'audio', 'pdf'].includes(state.type)) {
        const src = escape(url(item))
        body.innerHTML = state.type === 'image' ? `<img class="preview-image" src="${src}" alt="${escape(item.name)}">` : state.type === 'pdf' ? `<iframe class="preview-document-frame" src="${src}" title="${escape(item.name)}"></iframe>` : `<${state.type} controls preload="metadata" src="${src}"></${state.type}>`
        body.setAttribute('aria-busy', 'false'); return
      }
      if (state.type === 'unknown') throw new Error(t('This file type has no preview yet. Download it to open locally.'))
      const isArchive = officeExtensions.has(state.type) || state.type === 'zip'
      const result = await bytes(item, isArchive ? ZIP_LIMIT : TEXT_LIMIT, state.controller.signal, !isArchive)
      if (isArchive) {
        const parsed = await archive(result.data, state.type, state.controller.signal)
        if (active !== state) { parsed.urls?.forEach(URL.revokeObjectURL); return }
        Object.assign(state, parsed)
      }
      else { state.text = decode(result.data, result.truncated); state.truncated = result.truncated }
      draw(state)
    } catch (error) {
      if (active !== state || error.name === 'AbortError') return
      body.setAttribute('aria-busy', 'false')
      const message = typeof error.code === 'number' ? t('This document is damaged or unsupported.') : error.message || t('File preview could not load.')
      body.innerHTML = `<div class="preview-error" role="alert"><p>${escape(message)}</p><button type="button" class="ghost" data-preview-retry>${t('Retry preview')}</button></div>`
    }
  }
  function close(restoreFocus = true) {
    if (!active) return
    const state = active; active = undefined
    state.controller.abort()
    state.urls?.forEach(URL.revokeObjectURL)
    const dialog = document.querySelector('#file-preview-dialog')
    dialog.hidden = true; dialog.innerHTML = ''
    if (restoreFocus && state.trigger?.isConnected) state.trigger.focus()
  }
  document.addEventListener('click', event => {
    const trigger = event.target.closest('[data-file-preview]')
    if (trigger) { void open(JSON.parse(trigger.dataset.filePreview), trigger); return }
    if (!active) return
    if (event.target.id === 'file-preview-dialog' || event.target.closest('[data-preview-close]')) close()
    else if (event.target.closest('[data-preview-retry]')) void open(active.item, active.trigger)
    else if (event.target.closest('[data-preview-mode]')) { active.mode = event.target.closest('[data-preview-mode]').dataset.previewMode; draw(active) }
  })
  document.addEventListener('change', event => {
    if (!active || event.target.id !== 'file-preview-section') return
    active.section = Number(event.target.value)
    if (active.deck) document.querySelector(`#file-preview-body [data-slide="${active.section}"]`)?.scrollIntoView({ block: 'start' })
    else draw(active)
  })
  document.addEventListener('keydown', event => {
    if (!active) return
    if (event.key === 'Escape') { event.preventDefault(); event.stopImmediatePropagation(); close(); return }
    if (event.key !== 'Tab') return
    const focusable = [...document.querySelectorAll('#file-preview-dialog button, #file-preview-dialog a[href], #file-preview-dialog select, #file-preview-dialog iframe, #file-preview-dialog [controls]')].filter(el => !el.disabled)
    const first = focusable[0], last = focusable.at(-1)
    if (event.shiftKey && (document.activeElement === first || !document.querySelector('#file-preview-dialog').contains(document.activeElement))) { event.preventDefault(); last?.focus() }
    else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus() }
  }, true)
  window.addEventListener('pagehide', () => close(false))
  return { kind, url, button, inlineHtml, fillInline, open, close }
})()
