// Office documents drawn from their own XML: slides at their real positions, sizes and colors,
// Word documents as pages with their fonts, colors, lists, pictures and tables. Nothing in a
// document runs; pictures become blob: URLs that the preview revokes when it closes.
'use strict'

const OfficePreview = (() => {
  const IMAGE_TYPES = { png: 'image/png', jpg: 'image/jpeg', jpeg: 'image/jpeg', jpe: 'image/jpeg', gif: 'image/gif', bmp: 'image/bmp', webp: 'image/webp', svg: 'image/svg+xml' }
  const THEME = { dk1: '000000', lt1: 'FFFFFF', dk2: '1F497D', lt2: 'EEECE1', accent1: '4F81BD', accent2: 'C0504D', accent3: '9BBB59', accent4: '8064A2', accent5: '4BACC6', accent6: 'F79646', hlink: '0000FF', folHlink: '800080' }
  const PRESET = { black: '000000', white: 'FFFFFF', red: 'FF0000', green: '008000', blue: '0000FF', yellow: 'FFFF00', gray: '808080', grey: '808080', orange: 'FFA500', purple: '800080', navy: '000080', silver: 'C0C0C0', darkGray: 'A9A9A9', lightGray: 'D3D3D3' }
  const HIGHLIGHT = { yellow: 'FFFF00', green: '00FF00', cyan: '00FFFF', magenta: 'FF00FF', blue: '0000FF', red: 'FF0000', darkBlue: '000080', darkCyan: '008080', darkGreen: '008000', darkMagenta: '800080', darkRed: '800000', darkYellow: '808000', darkGray: '808080', lightGray: 'C0C0C0', black: '000000', white: 'FFFFFF' }
  // Preset shapes beyond rectangles and ovals, as outlines; borders follow only rectangles.
  const CLIP = {
    triangle: 'polygon(50% 0,100% 100%,0 100%)', rtTriangle: 'polygon(0 0,100% 100%,0 100%)', diamond: 'polygon(50% 0,100% 50%,50% 100%,0 50%)',
    parallelogram: 'polygon(25% 0,100% 0,75% 100%,0 100%)', trapezoid: 'polygon(25% 0,75% 0,100% 100%,0 100%)', pentagon: 'polygon(50% 0,100% 38%,82% 100%,18% 100%,0 38%)',
    homePlate: 'polygon(0 0,80% 0,100% 50%,80% 100%,0 100%)', chevron: 'polygon(0 0,80% 0,100% 50%,80% 100%,0 100%,20% 50%)',
    hexagon: 'polygon(25% 0,75% 0,100% 50%,75% 100%,25% 100%,0 50%)', octagon: 'polygon(29% 0,71% 0,100% 29%,100% 71%,71% 100%,29% 100%,0 71%,0 29%)',
    rightArrow: 'polygon(0 25%,70% 25%,70% 0,100% 50%,70% 100%,70% 75%,0 75%)', leftArrow: 'polygon(30% 0,30% 25%,100% 25%,100% 75%,30% 75%,30% 100%,0 50%)',
    upArrow: 'polygon(50% 0,100% 30%,75% 30%,75% 100%,25% 100%,25% 30%,0 30%)', downArrow: 'polygon(25% 0,75% 0,75% 70%,100% 70%,50% 100%,0 70%,25% 70%)',
    star5: 'polygon(50% 0,61% 35%,98% 35%,68% 57%,79% 91%,50% 70%,21% 91%,32% 57%,2% 35%,39% 35%)', plus: 'polygon(35% 0,65% 0,65% 35%,100% 35%,100% 65%,65% 65%,65% 100%,35% 100%,35% 65%,0 65%,0 35%,35% 35%)'
  }
  const LINES = new Set(['line', 'straightConnector1', 'bentConnector2', 'bentConnector3', 'curvedConnector3'])

  const kids = (node, name) => node ? [...node.children].filter(n => !name || n.localName === name) : []
  const kid = (node, name) => node ? [...node.children].find(n => n.localName === name) : undefined
  const path = (node, ...names) => names.reduce((n, name) => kid(n, name), node)
  const all = (node, name) => node ? [...node.getElementsByTagNameNS('*', name)] : []
  const attr = (node, name) => { for (const a of node?.attributes ?? []) if (a.localName === name) return a.value; return null }
  // r:id and r:embed: the relationship attribute, never a plain id="256" beside it.
  const rel = (node, name) => { for (const a of node?.attributes ?? []) if (a.localName === name && a.prefix) return a.value; return null }
  const num = (node, name, fallback) => { const raw = attr(node, name), value = Number(raw); return raw !== null && raw !== '' && Number.isFinite(value) ? value : fallback }
  const on = node => Boolean(node) && !['0', 'false', 'off', 'none'].includes(attr(node, 'val') ?? 'true')
  const hex = value => /^[0-9a-f]{6}$/i.test(value || '') ? value.toUpperCase() : null
  const pc = value => `${(value * 100).toFixed(4)}%`
  const damaged = () => new Error(t('This document is damaged or unsupported.'))

  function resolve(from, target) {
    if (target.startsWith('/')) return target.slice(1)
    const parts = from.split('/').slice(0, -1)
    for (const piece of target.split('/')) {
      if (piece === '..') parts.pop()
      else if (piece && piece !== '.') parts.push(piece)
    }
    return parts.join('/')
  }

  function open(files) {
    const docs = new Map(), relations = new Map(), images = new Map(), urls = []
    const doc = part => {
      if (!part || !files[part]) return null
      if (!docs.has(part)) {
        const parsed = new DOMParser().parseFromString(new TextDecoder().decode(files[part]), 'application/xml')
        if (parsed.querySelector('parsererror') || parsed.doctype) throw damaged()
        docs.set(part, parsed)
      }
      return docs.get(part)
    }
    const rels = part => {
      if (!relations.has(part)) {
        const map = new Map()
        for (const r of all(doc(part.replace(/([^/]*)$/, '_rels/$1.rels')), 'Relationship')) {
          if (attr(r, 'TargetMode') !== 'External') map.set(attr(r, 'Id'), { type: attr(r, 'Type') || '', part: resolve(part, attr(r, 'Target') || '') })
        }
        relations.set(part, map)
      }
      return relations.get(part)
    }
    const related = (part, type) => [...rels(part).values()].find(r => r.type.endsWith(`/${type}`))?.part
    const image = part => {
      if (!images.has(part)) {
        const type = IMAGE_TYPES[String(part).split('.').pop().toLowerCase()]
        const url = type && files[part] ? URL.createObjectURL(new Blob([files[part]], { type })) : ''
        if (url) urls.push(url)
        images.set(part, url)
      }
      return images.get(part)
    }
    return { doc, rels, related, image, urls }
  }

  // Colors: DrawingML's scheme, system and preset colors with their tint/shade/luminance changes.
  const clamp = v => Math.max(0, Math.min(255, Math.round(v)))
  const rgb = value => { const n = parseInt(value, 16); return [n >> 16 & 255, n >> 8 & 255, n & 255] }
  function hsl([r, g, b]) {
    r /= 255; g /= 255; b /= 255
    const max = Math.max(r, g, b), min = Math.min(r, g, b), l = (max + min) / 2, d = max - min
    if (!d) return [0, 0, l]
    const s = d / (1 - Math.abs(2 * l - 1))
    const h = max === r ? ((g - b) / d) % 6 : max === g ? (b - r) / d + 2 : (r - g) / d + 4
    return [h * 60, s, l]
  }
  function fromHsl([h, s, l]) {
    const c = (1 - Math.abs(2 * l - 1)) * s, x = c * (1 - Math.abs(((h / 60) % 2 + 2) % 2 - 1)), m = l - c / 2
    const [r, g, b] = h < 60 ? [c, x, 0] : h < 120 ? [x, c, 0] : h < 180 ? [0, c, x] : h < 240 ? [0, x, c] : h < 300 ? [x, 0, c] : [c, 0, x]
    return [(r + m) * 255, (g + m) * 255, (b + m) * 255]
  }
  function modified(base, node) {
    let c = rgb(base), alpha = 1
    for (const m of kids(node)) {
      const v = num(m, 'val', 100000) / 100000
      if (m.localName === 'tint') c = c.map(x => x + (255 - x) * (1 - v))
      else if (m.localName === 'shade') c = c.map(x => x * v)
      else if (m.localName === 'alpha') alpha = v
      else if (['lumMod', 'lumOff', 'satMod'].includes(m.localName)) {
        const h = hsl(c)
        if (m.localName === 'lumMod') h[2] *= v
        else if (m.localName === 'lumOff') h[2] += v
        else h[1] *= v
        h[1] = Math.min(1, Math.max(0, h[1])); h[2] = Math.min(1, Math.max(0, h[2]))
        c = fromHsl(h)
      }
    }
    c = c.map(clamp)
    return alpha < 1 ? `rgba(${c.join(',')},${alpha.toFixed(3)})` : `rgb(${c.join(',')})`
  }
  const tint = (base, v) => `rgb(${rgb(base).map(x => clamp(x + (255 - x) * (1 - v))).join(',')})`
  function color(parent, theme) {
    const c = kids(parent).find(n => ['srgbClr', 'schemeClr', 'sysClr', 'prstClr', 'scrgbClr'].includes(n.localName))
    if (!c) return null
    let base = null
    if (c.localName === 'srgbClr') base = hex(attr(c, 'val'))
    else if (c.localName === 'sysClr') base = hex(attr(c, 'lastClr')) || (attr(c, 'val') === 'window' ? 'FFFFFF' : '000000')
    else if (c.localName === 'schemeClr') { const v = attr(c, 'val'); base = theme.colors[theme.map[v] || v] || null }
    else if (c.localName === 'prstClr') base = PRESET[attr(c, 'val')] || null
    else base = ['r', 'g', 'b'].map(k => clamp(num(c, k, 0) / 100000 * 255).toString(16).padStart(2, '0')).join('').toUpperCase()
    return base && modified(base, c)
  }
  function fontList(names) {
    const clean = [...new Set(names.map(n => String(n || '').replace(/["'\\;:<>{}()]/g, '').trim()).filter(Boolean))]
    return clean.length ? `${clean.map(n => `'${n}'`).join(',')},sans-serif` : ''
  }

  // ---------------------------------------------------------------------------------------
  // Presentations

  function themeOf(pk, part) {
    const d = pk.doc(part), colors = { ...THEME }
    for (const c of kids(all(d, 'clrScheme')[0])) {
      const value = hex(attr(kid(c, 'srgbClr'), 'val')) || hex(attr(kid(c, 'sysClr'), 'lastClr'))
      if (value) colors[c.localName] = value
    }
    const font = (group, script) => attr(kid(all(d, group)[0], script), 'typeface') || ''
    return { colors, map: { bg1: 'lt1', tx1: 'dk1', bg2: 'lt2', tx2: 'dk2' }, fonts: { 'mj-lt': font('majorFont', 'latin'), 'mj-ea': font('majorFont', 'ea'), 'mn-lt': font('minorFont', 'latin'), 'mn-ea': font('minorFont', 'ea') } }
  }
  const nvOf = el => kids(el).find(n => n.localName.startsWith('nv'))
  const phOf = el => path(nvOf(el), 'nvPr', 'ph')
  const phGroup = type => ({ ctrTitle: 'title', title: 'title', subTitle: 'body', body: 'body', obj: 'body' })[type] || type
  const xfrmOf = el => path(el, 'spPr', 'xfrm') || kid(el, 'xfrm')

  function fillOf(node, ctx, rels) {
    for (const f of kids(node)) {
      switch (f.localName) {
        case 'noFill': return 'transparent'
        case 'solidFill': return color(f, ctx.theme) || 'transparent'
        case 'gradFill': {
          const stops = kids(kid(f, 'gsLst'), 'gs').map(g => [color(g, ctx.theme), num(g, 'pos', 0) / 1000]).filter(s => s[0]).sort((a, b) => a[1] - b[1])
          if (stops.length < 2) return stops[0]?.[0] || 'transparent'
          const list = stops.map(([c, p]) => `${c} ${p}%`).join(',')
          return kid(f, 'path') ? `radial-gradient(circle,${list})` : `linear-gradient(${(num(kid(f, 'lin'), 'ang', 0) / 60000 + 90) % 360}deg,${list})`
        }
        case 'blipFill': { const url = blipUrl(f, ctx, rels); return url ? `url('${url}') center/cover no-repeat` : 'transparent' }
        case 'pattFill': return color(kid(f, 'bgClr'), ctx.theme) || 'transparent'
      }
    }
    return null
  }
  function blipUrl(fill, ctx, rels) {
    const target = rels.get(rel(kid(fill, 'blip'), 'embed'))
    return target ? ctx.pk.image(target.part) : ''
  }
  function lineOf(sources, style, ctx) {
    for (const ln of sources) {
      if (!ln) continue
      if (kid(ln, 'noFill')) return null
      const c = color(kid(ln, 'solidFill'), ctx.theme) || color(all(ln, 'gs')[0], ctx.theme)
      if (c) return { color: c, width: num(ln, 'w', 9525), dash: attr(kid(ln, 'prstDash'), 'val') }
    }
    const ref = kid(style, 'lnRef'), c = num(ref, 'idx', 0) > 0 && color(ref, ctx.theme)
    return c ? { color: c, width: num(sources.find(Boolean), 'w', 9525), dash: null } : null
  }
  const border = (line, ctx) => line ? `border:max(1px,${ctx.u(line.width)}) ${line.dash && line.dash !== 'solid' ? (/dot/i.test(line.dash) ? 'dotted' : 'dashed') : 'solid'} ${line.color};` : ''

  // A placeholder takes what it leaves out from the same placeholder on its layout and master.
  function inherited(el, ctx) {
    const ph = phOf(el)
    if (!ph || ctx.decor) return []
    const type = attr(ph, 'type') || 'obj', idx = attr(ph, 'idx'), out = []
    const shapes = node => node ? kids(node).filter(s => phOf(s)) : []
    const onLayout = shapes(ctx.layoutTree)
    const layout = (idx !== null && onLayout.find(s => attr(phOf(s), 'idx') === idx)) || onLayout.find(s => phGroup(attr(phOf(s), 'type') || 'obj') === phGroup(type))
    if (layout) out.push({ node: layout, rels: ctx.layoutRels })
    const master = shapes(ctx.masterTree).find(s => phGroup(attr(phOf(s), 'type') || 'obj') === phGroup(type))
    if (master) out.push({ node: master, rels: ctx.masterRels })
    return out
  }
  function placed(el, chain, map) {
    for (const node of [el, ...chain.map(c => c.node)]) {
      const x = xfrmOf(node), off = kid(x, 'off'), ext = kid(x, 'ext')
      if (off && ext) return { box: map(num(off, 'x', 0), num(off, 'y', 0), num(ext, 'cx', 0), num(ext, 'cy', 0)), rot: num(x, 'rot', 0), flipH: attr(x, 'flipH') === '1', flipV: attr(x, 'flipV') === '1' }
    }
    return null
  }
  function boxCss(place, ctx, flips) {
    const [x, y, w, h] = place.box, turns = []
    if (place.rot) turns.push(`rotate(${(place.rot / 60000).toFixed(2)}deg)`)
    if (flips && place.flipH) turns.push('scaleX(-1)')
    if (flips && place.flipV) turns.push('scaleY(-1)')
    return `left:${pc(x / ctx.W)};top:${pc(y / ctx.H)};width:${pc(w / ctx.W)};height:${pc(h / ctx.H)};${turns.length ? `transform:${turns.join(' ')};` : ''}`
  }
  function outline(spPrs, w, h, ctx) {
    const geom = spPrs.map(s => kid(s, 'prstGeom')).find(Boolean), prst = attr(geom, 'prst') || 'rect'
    if (prst === 'ellipse') return { prst, css: 'border-radius:50%;' }
    if (['roundRect', 'round2SameRect', 'flowChartAlternateProcess'].includes(prst)) {
      const adj = all(geom, 'gd').find(g => attr(g, 'name') === 'adj' || attr(g, 'name') === 'adj1')
      const ratio = Math.min(0.5, Number(String(attr(adj, 'fmla') || '').replace(/^val\s+/, '')) / 100000 || 0.16667)
      return { prst, css: `border-radius:${ctx.u(Math.min(w, h) * ratio)};` }
    }
    return { prst, css: CLIP[prst] ? `clip-path:${CLIP[prst]};` : '' }
  }

  function tree(node, ctx, map) {
    let out = ''
    for (const child of kids(node)) {
      const items = child.localName === 'AlternateContent' ? kids(kid(child, 'Fallback') || kid(child, 'Choice')) : [child]
      for (const item of items) {
        if (attr(kid(nvOf(item), 'cNvPr'), 'hidden') === '1') continue
        if (item.localName === 'sp' || item.localName === 'cxnSp') out += shape(item, ctx, map)
        else if (item.localName === 'pic') out += picture(item, ctx, map)
        else if (item.localName === 'grpSp') out += group(item, ctx, map)
        else if (item.localName === 'graphicFrame') out += frame(item, ctx, map)
      }
    }
    return out
  }
  function group(el, ctx, map) {
    const x = path(el, 'grpSpPr', 'xfrm')
    if (!x) return tree(el, ctx, map)
    const off = kid(x, 'off'), ext = kid(x, 'ext'), chOff = kid(x, 'chOff'), chExt = kid(x, 'chExt')
    const ox = num(off, 'x', 0), oy = num(off, 'y', 0), ow = num(ext, 'cx', 0), oh = num(ext, 'cy', 0)
    const cx = num(chOff, 'x', ox), cy = num(chOff, 'y', oy), sx = ow / (num(chExt, 'cx', ow) || ow || 1), sy = oh / (num(chExt, 'cy', oh) || oh || 1)
    return tree(el, ctx, (a, b, w, h) => map(ox + (a - cx) * sx, oy + (b - cy) * sy, w * sx, h * sy))
  }
  function shape(el, ctx, map) {
    if (ctx.decor && phOf(el)) return ''
    const chain = inherited(el, ctx), style = kid(el, 'style')
    const spPrs = [kid(el, 'spPr'), ...chain.map(c => kid(c.node, 'spPr'))]
    let fill = null
    for (const [i, spPr] of spPrs.entries()) if (spPr && (fill = fillOf(spPr, ctx, i ? chain[i - 1].rels : ctx.rels))) break
    if (!fill) { const ref = kid(style, 'fillRef'); if (num(ref, 'idx', 0) > 0) fill = color(ref, ctx.theme) }
    const line = lineOf(spPrs.map(s => kid(s, 'ln')), style, ctx)
    const fontColor = color(kid(style, 'fontRef'), ctx.theme)
    const text = textBody(kid(el, 'txBody'), chain, ctx, { fontColor })
    let place = placed(el, chain, map)
    if (!place) { if (!text) return ''; place = { box: [0, 0, ctx.W, ctx.H] } }
    const [, , w, h] = place.box, shapeOutline = outline(spPrs, w, h, ctx)
    if (el.localName === 'cxnSp' || LINES.has(shapeOutline.prst)) return connector(place, line, ctx)
    const back = (fill && fill !== 'transparent') || line ? `<div class="pptx-fill" style="background:${fill || 'transparent'};${border(line, ctx)}${shapeOutline.css}"></div>` : ''
    if (!back && !text) return ''
    return `<div class="pptx-el" style="${boxCss(place, ctx, !text)}">${back}${text}</div>`
  }
  function connector(place, line, ctx) {
    if (!line) return ''
    const [x, y, w, h] = place.box, stroke = `max(1px,${ctx.u(line.width)})`
    const style = line.dash && line.dash !== 'solid' ? (/dot/i.test(line.dash) ? 'dotted' : 'dashed') : 'solid'
    // Straight lines are most connectors; a diagonal one is drawn corner to corner.
    if (h < w / 50 || w < h / 50) {
      const horizontal = h < w / 50
      return `<div class="pptx-el" style="${boxCss({ ...place, box: horizontal ? [x, y + h / 2, w, 0] : [x + w / 2, y, 0, h] }, ctx, false)}border-${horizontal ? 'top' : 'left'}:${stroke} ${style} ${line.color};"></div>`
    }
    const down = place.flipV === place.flipH
    return `<svg class="pptx-el" style="${boxCss({ ...place, flipH: false, flipV: false }, ctx, false)}overflow:visible" viewBox="0 0 100 100" preserveAspectRatio="none"><line x1="0" y1="${down ? 0 : 100}" x2="100" y2="${down ? 100 : 0}" stroke="${line.color}" stroke-width="${Math.max(1, line.width / 9525).toFixed(1)}" vector-effect="non-scaling-stroke"/></svg>`
  }
  function picture(el, ctx, map) {
    if (ctx.decor && phOf(el)) return ''
    const place = placed(el, inherited(el, ctx), map)
    if (!place) return ''
    const blip = kid(el, 'blipFill'), url = blipUrl(blip, ctx, ctx.rels), crop = kid(blip, 'srcRect')
    const [l, t0, r, b] = ['l', 't', 'r', 'b'].map(k => num(crop, k, 0) / 100000), sx = 1 - l - r, sy = 1 - t0 - b
    const spPr = kid(el, 'spPr'), [, , w, h] = place.box, line = lineOf([kid(spPr, 'ln')], null, ctx)
    const alt = attr(kid(nvOf(el), 'cNvPr'), 'descr') || ''
    const inner = url && sx > 0 && sy > 0
      ? `<img src="${url}" alt="${escape(alt)}" loading="lazy" style="left:${pc(-l / sx)};top:${pc(-t0 / sy)};width:${pc(1 / sx)};height:${pc(1 / sy)}">`
      : `<span class="pptx-missing">${escape(t('Image not shown'))}</span>`
    return `<div class="pptx-el pptx-pic" style="${boxCss(place, ctx, true)}${outline([spPr], w, h, ctx).css}${border(line, ctx)}">${inner}</div>`
  }
  function frame(el, ctx, map) {
    const place = placed(el, inherited(el, ctx), map)
    if (!place) return ''
    const tbl = all(el, 'tbl')[0]
    if (!tbl) {
      const uri = attr(path(el, 'graphic', 'graphicData'), 'uri') || ''
      const label = uri.includes('chart') ? 'Chart not shown' : uri.includes('diagram') ? 'Diagram not shown' : 'Object not shown'
      return `<div class="pptx-el" style="${boxCss(place, ctx, false)}"><span class="pptx-missing">${escape(t(label))}</span></div>`
    }
    return `<div class="pptx-el" style="${boxCss(place, ctx, false)}">${table(tbl, ctx)}</div>`
  }
  function table(tbl, ctx) {
    const cols = kids(kid(tbl, 'tblGrid'), 'gridCol').map(c => num(c, 'w', 0)), total = cols.reduce((a, b) => a + b, 0) || 1
    const pr = kid(tbl, 'tblPr'), styleId = kid(pr, 'tableStyleId')?.textContent.trim()
    // Without its own style a table uses PowerPoint's default, Medium Style 2 in accent 1.
    const styled = !styleId || styleId === '{5C22544A-7EE6-4342-B048-85BDC9FD1C3A}'
    const firstRow = attr(pr, 'firstRow') === '1', banded = attr(pr, 'bandRow') === '1', accent = ctx.theme.colors.accent1
    const edge = (tcPr, side) => {
      const ln = kid(tcPr, side)
      if (ln) { const line = lineOf([ln], null, ctx); return line ? `max(1px,${ctx.u(line.width)}) solid ${line.color}` : 'none' }
      return styled ? `max(1px,${ctx.u(12700)}) solid #fff` : 'none'
    }
    const rows = kids(tbl, 'tr').map((tr, r) => {
      const header = styled && firstRow && r === 0
      const cells = kids(tr, 'tc').filter(tc => attr(tc, 'hMerge') !== '1' && attr(tc, 'vMerge') !== '1').map(tc => {
        const tcPr = kid(tc, 'tcPr')
        let bg = fillOf(tcPr, ctx, ctx.rels)
        if (!bg && styled) bg = header ? `#${accent}` : tint(accent, banded && (r - (firstRow ? 1 : 0)) % 2 === 0 ? 0.4 : 0.2)
        const span = num(tc, 'gridSpan', 1), down = num(tc, 'rowSpan', 1), anchor = attr(tcPr, 'anchor')
        const pad = `${ctx.u(num(tcPr, 'marT', 45720))} ${ctx.u(num(tcPr, 'marR', 91440))} ${ctx.u(num(tcPr, 'marB', 45720))} ${ctx.u(num(tcPr, 'marL', 91440))}`
        const body = kid(tc, 'txBody')
        const text = body ? paragraphs(body, [kid(body, 'lstStyle')].filter(Boolean), [kid(ctx.txStyles, 'otherStyle')].filter(Boolean), ctx, { fontColor: header ? '#fff' : null, bold: header, scale: 1, lnReduce: 0 }) : ''
        return `<td${span > 1 ? ` colspan="${span}"` : ''}${down > 1 ? ` rowspan="${down}"` : ''} style="background:${bg || 'transparent'};border-left:${edge(tcPr, 'lnL')};border-right:${edge(tcPr, 'lnR')};border-top:${edge(tcPr, 'lnT')};border-bottom:${edge(tcPr, 'lnB')};padding:${pad};vertical-align:${{ ctr: 'middle', b: 'bottom' }[anchor] || 'top'}">${text}</td>`
      }).join('')
      return `<tr style="height:${ctx.u(num(tr, 'h', 0))}">${cells}</tr>`
    }).join('')
    return `<table class="pptx-table"><colgroup>${cols.map(w => `<col style="width:${pc(w / total)}">`).join('')}</colgroup>${rows}</table>`
  }

  function textBody(body, chain, ctx, options) {
    if (!body || !all(body, 't').some(n => n.textContent)) return ''
    const bodyPrs = [kid(body, 'bodyPr'), ...chain.map(c => path(c.node, 'txBody', 'bodyPr'))].filter(Boolean)
    const prop = name => { for (const b of bodyPrs) { const v = attr(b, name); if (v !== null) return v } return null }
    const inset = (name, fallback) => { const v = Number(prop(name)); return prop(name) !== null && Number.isFinite(v) ? v : fallback }
    const ph = phOf(body.parentNode)
    const key = ph ? ({ title: 'titleStyle', body: 'bodyStyle' })[phGroup(attr(ph, 'type') || 'obj')] || 'otherStyle' : 'otherStyle'
    const own = [kid(body, 'lstStyle'), ...chain.map(c => path(c.node, 'txBody', 'lstStyle'))].filter(Boolean)
    const fit = bodyPrs.map(b => kid(b, 'normAutofit')).find(Boolean)
    const anchor = prop('anchor') || 't'
    const css = `padding:${ctx.u(inset('tIns', 45720))} ${ctx.u(inset('rIns', 91440))} ${ctx.u(inset('bIns', 45720))} ${ctx.u(inset('lIns', 91440))};justify-content:${{ ctr: 'center', b: 'flex-end' }[anchor] || 'flex-start'};${prop('wrap') === 'none' ? 'white-space:pre;' : ''}${['vert', 'eaVert', 'wordArtVert', 'vert270'].includes(prop('vert')) ? 'writing-mode:vertical-rl;' : ''}`
    const html = paragraphs(body, own, [kid(ctx.txStyles, key)].filter(Boolean), ctx, { ...options, scale: num(fit, 'fontScale', 100000) / 100000, lnReduce: num(fit, 'lnSpcReduction', 0) / 100000 })
    return `<div class="pptx-text" style="${css}">${html}</div>`
  }
  const BULLETS = new Set(['buNone', 'buChar', 'buAutoNum', 'buBlip'])
  const SYMBOL_BULLETS = { 'Ø': '➢', '§': '▪', 'ü': '✓', 'l': '●', 'q': '❑', 'n': '■', 'Ÿ': '•', 'v': '❖', 'o': '◦' }
  function autoNumber(n, scheme) {
    const roman = value => [[1000, 'M'], [900, 'CM'], [500, 'D'], [400, 'CD'], [100, 'C'], [90, 'XC'], [50, 'L'], [40, 'XL'], [10, 'X'], [9, 'IX'], [5, 'V'], [4, 'IV'], [1, 'I']].reduce((out, [v, s]) => { while (value >= v) { out += s; value -= v } return out }, '')
    const letter = value => String.fromCharCode(97 + (value - 1) % 26)
    const body = /^alphaLc/.test(scheme) ? letter(n) : /^alphaUc/.test(scheme) ? letter(n).toUpperCase() : /^romanLc/.test(scheme) ? roman(n).toLowerCase() : /^romanUc/.test(scheme) ? roman(n) : /^circleNum/.test(scheme) && n <= 20 ? String.fromCharCode(0x245f + n) : String(n)
    return /ParenBoth$/.test(scheme) ? `(${body})` : /ParenR$/.test(scheme) ? `${body})` : /Plain$/.test(scheme) || /^circleNum/.test(scheme) ? body : `${body}.`
  }
  function paragraphs(body, own, base, ctx, options) {
    const counters = []
    return kids(body, 'p').map(p => {
      const pPr = kid(p, 'pPr'), lvl = Math.min(9, num(pPr, 'lvl', 0) + 1)
      const ownLevels = [pPr, ...own.map(s => kid(s, `lvl${lvl}pPr`))].filter(Boolean), baseLevels = base.map(s => kid(s, `lvl${lvl}pPr`)).filter(Boolean)
      const levels = [...ownLevels, ...baseLevels]
      const pick = fn => { for (const l of levels) { const v = fn(l); if (v !== null && v !== undefined) return v } return null }
      const runs = rPr => ({ all: [rPr, ...levels.map(l => kid(l, 'defRPr'))].filter(Boolean), colored: [rPr, ...ownLevels.map(l => kid(l, 'defRPr'))].filter(Boolean), base: baseLevels.map(l => kid(l, 'defRPr')).filter(Boolean) })
      const sizeOf = chain => { for (const c of chain) { const v = num(c, 'sz', null); if (v !== null) return v / 100 * options.scale } return 18 * options.scale }
      const first = kids(p).find(n => ['r', 'fld'].includes(n.localName))
      const pt = sizeOf(runs(kid(first, 'rPr') ?? kid(p, 'endParaRPr')).all)
      const content = kids(p).map(n => ['r', 'fld'].includes(n.localName) ? run(n, runs(kid(n, 'rPr')), ctx, options, sizeOf) : n.localName === 'br' ? '<br>' : '').join('')
      const spacing = (name, fallback) => {
        const node = pick(l => kid(l, name) ?? null)
        if (kid(node, 'spcPts')) return ctx.u(num(kid(node, 'spcPts'), 'val', 0) / 100 * 12700)
        if (kid(node, 'spcPct')) return `${(num(kid(node, 'spcPct'), 'val', 0) / 100000 * 1.2).toFixed(3)}em`
        return fallback
      }
      const lnSpc = pick(l => kid(l, 'lnSpc') ?? null)
      const lineHeight = kid(lnSpc, 'spcPts') ? ctx.u(num(kid(lnSpc, 'spcPts'), 'val', 0) / 100 * 12700) : (num(kid(lnSpc, 'spcPct'), 'val', 100000) / 100000 * 1.2 * (1 - options.lnReduce)).toFixed(3)
      const marL = Number(pick(l => attr(l, 'marL')) || 0), indent = Number(pick(l => attr(l, 'indent')) || 0)
      let marker = ''
      const bullet = content ? pick(l => kids(l).find(c => BULLETS.has(c.localName)) ?? null) : null
      if (bullet?.localName === 'buAutoNum') {
        counters[lvl] = (counters[lvl] ?? num(bullet, 'startAt', 1) - 1) + 1
        counters.length = lvl + 1
        marker = autoNumber(counters[lvl], attr(bullet, 'type') || 'arabicPeriod')
      } else {
        if (content) counters.length = lvl
        if (bullet?.localName === 'buChar') {
          const char = attr(bullet, 'char') || '•', face = attr(pick(l => kid(l, 'buFont') ?? null), 'typeface') || ''
          marker = /wingdings|symbol/i.test(face) ? SYMBOL_BULLETS[char] || '•' : char
        } else if (bullet?.localName === 'buBlip') marker = '•'
      }
      const markerHtml = marker ? `<span class="pptx-bullet" style="min-width:${ctx.u(Math.max(0, -indent))};${(c => c ? `color:${c};` : '')(color(pick(l => kid(l, 'buClr') ?? null), ctx.theme))}font-size:${(num(pick(l => kid(l, 'buSzPct') ?? null), 'val', 100000) / 1000).toFixed(0)}%">${escape(marker)}</span>` : ''
      const align = { ctr: 'center', r: 'right', just: 'justify', dist: 'justify' }[pick(l => attr(l, 'algn'))] || 'left'
      return `<p style="text-align:${align};padding-left:${ctx.u(marL)};text-indent:${ctx.u(indent)};margin:${spacing('spcBef', '0')} 0 ${spacing('spcAft', '0')};line-height:${lineHeight};font-size:${ctx.u(pt * 12700)}">${markerHtml}${content || '<br>'}</p>`
    }).join('')
  }
  function run(r, chains, ctx, options, sizeOf) {
    const text = kid(r, 't')?.textContent ?? ''
    if (!text) return ''
    const get = name => { for (const c of chains.all) { const v = attr(c, name); if (v !== null) return v } return null }
    const child = name => { for (const c of chains.all) { const v = kid(c, name); if (v) return v } return null }
    const fillIn = list => { for (const c of list) { const f = kid(c, 'solidFill') || kid(c, 'gradFill'); if (f) return f.localName === 'solidFill' ? color(f, ctx.theme) : color(all(f, 'gs')[0], ctx.theme) } return null }
    const ink = fillIn(chains.colored) || options.fontColor || fillIn(chains.base) || `#${ctx.theme.colors[ctx.theme.map.tx1] || '000000'}`
    const underline = get('u'), strike = get('strike'), baseline = Number(get('baseline') || 0)
    const decoration = [underline && underline !== 'none' && 'underline', strike && strike !== 'noStrike' && 'line-through'].filter(Boolean)
    const face = name => { const f = attr(child(name), 'typeface') || ''; return f.startsWith('+') ? ctx.theme.fonts[f.slice(1)] || '' : f }
    const family = fontList([face('ea'), face('latin')]), highlight = color(child('highlight'), ctx.theme)
    const css = `font-size:${ctx.u(sizeOf(chains.all) * 12700)};color:${ink};${['1', 'true'].includes(get('b')) || options.bold ? 'font-weight:700;' : ''}${['1', 'true'].includes(get('i')) ? 'font-style:italic;' : ''}${decoration.length ? `text-decoration:${decoration.join(' ')};` : ''}${baseline ? `vertical-align:${baseline > 0 ? 'super' : 'sub'};font-size:${ctx.u(sizeOf(chains.all) * 12700 * 0.7)};` : ''}${get('cap') === 'all' ? 'text-transform:uppercase;' : get('cap') === 'small' ? 'font-variant:small-caps;' : ''}${family ? `font-family:${family};` : ''}${highlight ? `background:${highlight};` : ''}`
    return `<span style="${css}">${escape(text)}</span>`
  }
  function background(chain, ctx) {
    for (const [d, part] of chain) {
      const bg = d && path(d.documentElement, 'cSld', 'bg')
      if (!bg) continue
      const fill = kid(bg, 'bgPr') && fillOf(kid(bg, 'bgPr'), ctx, ctx.pk.rels(part))
      if (fill) return fill
      const ref = color(kid(bg, 'bgRef'), ctx.theme)
      if (ref) return ref
    }
    return `#${ctx.theme.colors[ctx.theme.map.bg1] || 'FFFFFF'}`
  }
  function slide(pk, part, W, H) {
    const d = pk.doc(part), rels = pk.rels(part)
    const layoutPart = pk.related(part, 'slideLayout'), layout = pk.doc(layoutPart)
    const masterPart = layoutPart && pk.related(layoutPart, 'slideMaster'), master = pk.doc(masterPart)
    const theme = themeOf(pk, masterPart && pk.related(masterPart, 'theme'))
    for (const a of all(master, 'clrMap')[0]?.attributes ?? []) theme.map[a.localName] = a.value
    const treeOf = doc => doc && path(doc.documentElement, 'cSld', 'spTree')
    const ctx = { pk, W, H, theme, u: emu => `${(emu * 100 / W).toFixed(4)}cqw`, txStyles: all(master, 'txStyles')[0], layoutTree: treeOf(layout), masterTree: treeOf(master), layoutRels: layoutPart ? pk.rels(layoutPart) : new Map(), masterRels: masterPart ? pk.rels(masterPart) : new Map() }
    const identity = (x, y, w, h) => [x, y, w, h]
    const showMaster = attr(d.documentElement, 'showMasterSp') !== '0'
    let html = ''
    if (showMaster && ctx.masterTree && attr(layout?.documentElement, 'showMasterSp') !== '0') html += tree(ctx.masterTree, { ...ctx, rels: ctx.masterRels, decor: true }, identity)
    if (showMaster && ctx.layoutTree) html += tree(ctx.layoutTree, { ...ctx, rels: ctx.layoutRels, decor: true }, identity)
    const own = treeOf(d)
    html += own ? tree(own, { ...ctx, rels }, identity) : `<div class="pptx-text" style="padding:5cqw">${all(d, 'p').map(p => `<p style="font-size:2.4cqw;margin-bottom:1cqw">${escape(all(p, 't').map(n => n.textContent).join(''))}</p>`).join('')}</div>`
    const family = fontList([theme.fonts['mn-ea'], theme.fonts['mn-lt']])
    return `<div class="pptx-slide" style="aspect-ratio:${W}/${H};background:${background([[d, part], [layout, layoutPart], [master, masterPart]], ctx)};${family ? `font-family:${family};` : ''}">${html}</div>`
  }
  function pptx(files) {
    const pk = open(files), main = 'ppt/presentation.xml', presentation = pk.doc(main)
    if (!presentation) throw damaged()
    const size = all(presentation, 'sldSz')[0], W = num(size, 'cx', 0) || 12192000, H = num(size, 'cy', 0) || 6858000
    const rels = pk.rels(main)
    const sections = all(presentation, 'sldId').slice(0, 100).map((id, index) => {
      const target = rels.get(rel(id, 'id'))
      if (!target || !pk.doc(target.part)) throw damaged()
      return { name: t('Slide {n}', { n: index + 1 }), html: slide(pk, target.part, W, H) }
    })
    return { sections, urls: pk.urls, deck: true }
  }

  // ---------------------------------------------------------------------------------------
  // Word documents

  const twipsPx = v => v / 15
  function bulletText(text) {
    const c = text.codePointAt(0) ?? 0
    if (text === 'o') return '◦'
    if (c >= 0xf000 && c <= 0xf0ff) return { 0xf0a7: '▪', 0xf0d8: '➢', 0xf0fc: '✓', 0xf076: '❖', 0xf06e: '■', 0xf0a8: '◆' }[c] || '•'
    return text || '•'
  }
  function wordNumber(n, fmt) {
    const chinese = value => {
      const digits = '零一二三四五六七八九'
      if (value < 10) return digits[value]
      if (value < 20) return `十${value % 10 ? digits[value % 10] : ''}`
      if (value < 100) return `${digits[Math.floor(value / 10)]}十${value % 10 ? digits[value % 10] : ''}`
      return String(value)
    }
    if (['chineseCounting', 'chineseCountingThousand', 'ideographTraditional', 'japaneseCounting', 'ideographDigital', 'taiwaneseCounting'].includes(fmt)) return chinese(n)
    if (fmt === 'decimalZero') return String(n).padStart(2, '0')
    if (fmt === 'decimalEnclosedCircle' || fmt === 'decimalEnclosedCircleChinese') return n <= 20 ? String.fromCharCode(0x245f + n) : String(n)
    if (/(Letter|Roman)$/.test(fmt)) return autoNumber(n, `${fmt.endsWith('Roman') ? 'roman' : 'alpha'}${fmt.startsWith('upper') ? 'Uc' : 'Lc'}Plain`)
    return String(n)
  }
  function marker(numId, ilvl, ctx) {
    const entry = ctx.nums.get(numId)
    const levels = entry && kids(entry, 'lvl')
    const lvl = levels?.find(l => attr(l, 'ilvl') === String(ilvl))
    if (!lvl || numId === '0') return { html: '', pPr: null }
    const counters = ctx.counters.get(numId) ?? []
    const start = l => num(kid(l, 'start'), 'val', 1)
    counters[ilvl] = (counters[ilvl] ?? start(lvl) - 1) + 1
    counters.length = ilvl + 1
    ctx.counters.set(numId, counters)
    const fmt = attr(kid(lvl, 'numFmt'), 'val') || 'decimal', text = attr(kid(lvl, 'lvlText'), 'val') ?? ''
    const label = fmt === 'bullet' ? bulletText(text) : fmt === 'none' ? '' : text.replace(/%([1-9])/g, (_, k) => {
      const at = Number(k) - 1, level = levels.find(l => attr(l, 'ilvl') === String(at))
      return wordNumber(counters[at] ?? start(level), attr(kid(level, 'numFmt'), 'val') || 'decimal')
    })
    return { html: label ? `<span class="docx-marker">${escape(label)}</span>` : '', pPr: kid(lvl, 'pPr') }
  }
  function blocks(node, ctx) {
    let out = ''
    for (const n of kids(node)) {
      if (n.localName === 'p') out += paragraph(n, ctx)
      else if (n.localName === 'tbl') out += wordTable(n, ctx)
      else if (n.localName === 'sdt') out += blocks(kid(n, 'sdtContent'), ctx)
      else if (['customXml', 'ins'].includes(n.localName)) out += blocks(n, ctx)
    }
    return out
  }
  function paragraph(p, ctx) {
    const pPr = kid(p, 'pPr'), styleId = attr(kid(pPr, 'pStyle'), 'val') || ctx.paraDefault
    const styleChain = ctx.chain(styleId)
    const find = (chain, name) => { for (const c of chain) { const n = kid(c, name); if (n) return n } return null }
    const numPr = find([pPr, ...styleChain.map(s => kid(s, 'pPr'))].filter(Boolean), 'numPr')
    const list = numPr ? marker(attr(kid(numPr, 'numId'), 'val'), num(kid(numPr, 'ilvl'), 'val', 0), ctx) : { html: '', pPr: null }
    const chain = [pPr, list.pPr, ...styleChain.map(s => kid(s, 'pPr')), ctx.defaults.pPr].filter(Boolean)
    const names = styleChain.map(s => attr(kid(s, 'name'), 'val') || '')
    const level = Number(names.map(n => n.match(/^heading\s*([1-9])$/i)?.[1]).find(Boolean) || (names.some(n => /^title$/i.test(n)) && 1) || String(styleId || '').match(/^(?:Heading|标题)\s*([1-6])$/i)?.[1] || 0)
    const tag = level ? `h${Math.min(6, level)}` : 'p'
    const ind = find(chain, 'ind'), spacing = find(chain, 'spacing'), jc = attr(find(chain, 'jc'), 'val'), shade = hex(attr(find(chain, 'shd'), 'fill'))
    const left = num(ind, 'left', num(ind, 'start', 0)), right = num(ind, 'right', num(ind, 'end', 0)), first = num(ind, 'firstLine', 0) - num(ind, 'hanging', 0)
    const line = num(spacing, 'line', 0), rule = attr(spacing, 'lineRule') || 'auto'
    const css = [
      jc && `text-align:${{ center: 'center', right: 'right', end: 'right', both: 'justify', distribute: 'justify' }[jc] || 'left'}`,
      left && `padding-left:${twipsPx(left).toFixed(1)}px`, right && `padding-right:${twipsPx(right).toFixed(1)}px`, first && `text-indent:${twipsPx(first).toFixed(1)}px`,
      spacing && attr(spacing, 'before') !== null && `margin-top:${twipsPx(num(spacing, 'before', 0)).toFixed(1)}px`,
      spacing && attr(spacing, 'after') !== null && `margin-bottom:${twipsPx(num(spacing, 'after', 0)).toFixed(1)}px`,
      line && (rule === 'auto' ? `line-height:${(line / 240 * 1.15).toFixed(3)}` : `line-height:${twipsPx(line).toFixed(1)}px`),
      shade && `background:#${shade}`
    ].filter(Boolean).join(';')
    const base = [...styleChain.map(s => kid(s, 'rPr')), ctx.defaults.rPr].filter(Boolean)
    const markerHtml = list.html && first < 0 ? list.html.replace('class="docx-marker"', `class="docx-marker" style="min-width:${twipsPx(-first).toFixed(1)}px"`) : list.html
    return `<${tag}${css ? ` style="${css}"` : ''}>${markerHtml}${inline(p, ctx, base) || '<br>'}</${tag}>`
  }
  function inline(node, ctx, base) {
    let out = ''
    for (const n of kids(node)) {
      if (n.localName === 'r') out += wordRun(n, ctx, base)
      else if (['hyperlink', 'ins', 'smartTag', 'fldSimple', 'customXml', 'dir', 'bdo'].includes(n.localName)) out += inline(n, ctx, base)
      else if (n.localName === 'sdt') out += inline(kid(n, 'sdtContent'), ctx, base)
      else if (['oMath', 'oMathPara'].includes(n.localName)) out += `<span class="docx-math">${escape(n.textContent)}</span>`
    }
    return out
  }
  function wordImage(n, ctx) {
    const blip = all(n, 'blip')[0], data = all(n, 'imagedata')[0]
    const target = ctx.rels.get(rel(blip, 'embed') || rel(data, 'id'))
    const url = target ? ctx.pk.image(target.part) : ''
    if (!url) return `<span class="docx-missing">${escape(t(all(n, 'chart').length ? 'Chart not shown' : 'Image not shown'))}</span>`
    const extent = all(n, 'extent')[0], width = num(extent, 'cx', 0) / 9525
    return `<img src="${url}" alt="${escape(attr(all(n, 'docPr')[0], 'descr') || '')}" loading="lazy"${width ? ` style="width:${width.toFixed(1)}px"` : ''}>`
  }
  function wordRun(r, ctx, base) {
    const rPr = kid(r, 'rPr'), runStyle = attr(kid(rPr, 'rStyle'), 'val')
    const chain = [rPr, ...(runStyle ? ctx.chain(runStyle).map(s => kid(s, 'rPr')) : []), ...base].filter(Boolean)
    const get = name => { for (const c of chain) { const n = kid(c, name); if (n) return n } return null }
    if (on(get('vanish'))) return ''
    let html = ''
    for (const n of kids(r)) {
      if (n.localName === 't') html += escape(n.textContent)
      else if (n.localName === 'tab') html += '\t'
      else if (['br', 'cr'].includes(n.localName)) html += '<br>'
      else if (n.localName === 'noBreakHyphen') html += '‑'
      else if (n.localName === 'sym') { const code = parseInt(attr(n, 'char') || '', 16); html += escape(code >= 0xf000 ? '•' : Number.isFinite(code) ? String.fromCodePoint(code) : '') }
      else if (['drawing', 'pict', 'object'].includes(n.localName)) html += wordImage(n, ctx)
    }
    if (!html) return ''
    const ink = hex(attr(get('color'), 'val')), size = num(get('sz'), 'val', 0), mark = HIGHLIGHT[attr(get('highlight'), 'val')] || hex(attr(get('shd'), 'fill'))
    const underline = attr(get('u'), 'val'), align = attr(get('vertAlign'), 'val'), fonts = get('rFonts')
    const decoration = [underline && underline !== 'none' && 'underline', (on(get('strike')) || on(get('dstrike'))) && 'line-through'].filter(Boolean)
    const family = fontList([attr(fonts, 'eastAsia'), attr(fonts, 'ascii') || attr(fonts, 'hAnsi')])
    const css = [ink && `color:#${ink}`, size && `font-size:${(size / 2).toFixed(1)}pt`, mark && `background:#${mark}`, decoration.length && `text-decoration:${decoration.join(' ')}`,
      align === 'superscript' && 'vertical-align:super;font-size:smaller', align === 'subscript' && 'vertical-align:sub;font-size:smaller',
      on(get('caps')) && 'text-transform:uppercase', on(get('smallCaps')) && 'font-variant:small-caps', family && `font-family:${family}`].filter(Boolean).join(';')
    if (css) html = `<span style="${css}">${html}</span>`
    if (on(get('i'))) html = `<em>${html}</em>`
    if (on(get('b'))) html = `<strong>${html}</strong>`
    return html
  }
  function wordTable(tbl, ctx) {
    const pr = kid(tbl, 'tblPr'), styleId = attr(kid(pr, 'tblStyle'), 'val')
    const borders = [kid(pr, 'tblBorders'), ...(styleId ? ctx.chain(styleId) : []).map(s => path(s, 'tblPr', 'tblBorders'))].find(Boolean)
    const borderless = borders && kids(borders).length && kids(borders).every(b => ['nil', 'none'].includes(attr(b, 'val')))
    const grid = kids(kid(tbl, 'tblGrid'), 'gridCol').map(c => num(c, 'w', 0)), total = grid.reduce((a, b) => a + b, 0)
    // Vertically merged cells: a "restart" cell grows by each "continue" cell below it.
    const rows = [], open = []
    for (const tr of kids(tbl, 'tr')) {
      const row = []
      let column = 0
      for (const tc of kids(tr, 'tc')) {
        const tcPr = kid(tc, 'tcPr'), span = Math.max(1, num(kid(tcPr, 'gridSpan'), 'val', 1)), merge = kid(tcPr, 'vMerge')
        if (merge && attr(merge, 'val') !== 'restart') { if (open[column]) open[column].rows++; column += span; continue }
        const cell = { tc, span, rows: 1 }
        open[column] = merge ? cell : null
        row.push(cell); column += span
      }
      rows.push(row)
    }
    const body = rows.map(row => `<tr>${row.map(cell => {
      const tcPr = kid(cell.tc, 'tcPr'), fill = hex(attr(kid(tcPr, 'shd'), 'fill')), valign = attr(kid(tcPr, 'vAlign'), 'val')
      const css = [fill && `background:#${fill}`, valign === 'center' && 'vertical-align:middle', valign === 'bottom' && 'vertical-align:bottom'].filter(Boolean).join(';')
      return `<td${cell.span > 1 ? ` colspan="${cell.span}"` : ''}${cell.rows > 1 ? ` rowspan="${cell.rows}"` : ''}${css ? ` style="${css}"` : ''}>${blocks(cell.tc, ctx)}</td>`
    }).join('')}</tr>`).join('')
    const columns = total ? `<colgroup>${grid.map(w => `<col style="width:${pc(w / total)}">`).join('')}</colgroup>` : ''
    return `<div class="docx-table-wrap"><table class="docx-table${borderless ? ' docx-borderless' : ''}"${total ? ` style="width:min(100%,${twipsPx(total).toFixed(0)}px)"` : ''}>${columns}${body}</table></div>`
  }
  function docx(files) {
    const pk = open(files), main = 'word/document.xml', body = all(pk.doc(main), 'body')[0]
    if (!body) throw damaged()
    const styleDoc = pk.doc('word/styles.xml'), styleList = all(styleDoc, 'style')
    const styles = new Map(styleList.map(s => [attr(s, 'styleId'), s]))
    const chain = id => { const out = []; let s = styles.get(id); while (s && out.length < 12 && !out.includes(s)) { out.push(s); s = styles.get(attr(kid(s, 'basedOn'), 'val')) } return out }
    const defaults = all(styleDoc, 'docDefaults')[0]
    const numbering = pk.doc('word/numbering.xml'), abstracts = new Map(all(numbering, 'abstractNum').map(a => [attr(a, 'abstractNumId'), a]))
    const nums = new Map(all(numbering, 'num').map(n => [attr(n, 'numId'), abstracts.get(attr(kid(n, 'abstractNumId'), 'val'))]))
    const paraDefault = attr(styleList.find(s => attr(s, 'type') === 'paragraph' && ['1', 'true', 'on'].includes(attr(s, 'default'))), 'styleId')
    const ctx = { pk, rels: pk.rels(main), chain, nums, counters: new Map(), paraDefault, defaults: { rPr: path(defaults, 'rPrDefault', 'rPr'), pPr: path(defaults, 'pPrDefault', 'pPr') } }
    const html = blocks(body, ctx)
    const section = kid(body, 'sectPr') || all(body, 'sectPr').pop(), size = kid(section, 'pgSz'), margin = kid(section, 'pgMar')
    const pad = (side, fallback) => Math.min(side === 'top' || side === 'bottom' ? 64 : 80, twipsPx(num(margin, side, fallback)))
    const normal = [...chain(paraDefault).map(s => kid(s, 'rPr')), ctx.defaults.rPr].filter(Boolean)
    const find = name => { for (const c of normal) { const n = kid(c, name); if (n) return n } return null }
    const fonts = find('rFonts'), family = fontList([attr(fonts, 'eastAsia'), attr(fonts, 'ascii')]), size2 = num(find('sz'), 'val', 21)
    const css = `max-width:${twipsPx(num(size, 'w', 11906)).toFixed(0)}px;padding:${pad('top', 1440).toFixed(0)}px ${pad('right', 1800).toFixed(0)}px ${pad('bottom', 1440).toFixed(0)}px ${pad('left', 1800).toFixed(0)}px;font-size:${(size2 / 2).toFixed(1)}pt;${family ? `font-family:${family};` : ''}`
    return { sections: [{ name: t('Document'), html: `<article class="docx-page${styleDoc ? ' docx-styled' : ''}" style="${css}">${html}</article>` }], urls: pk.urls, page: true }
  }

  return { pptx, docx }
})()
