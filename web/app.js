// dotbot's page: the team in the sidebar, a conversation or a team page in the middle,
// and a panel for a Bot's details, memory and live browser. It reads the team with
// team.state and then follows the feed (hello, team.patch, work, chat.item, chat.stream).
'use strict'

const $ = selector => document.querySelector(selector)
const COLORS = ['#13b99a', '#ff8946', '#578bea', '#ef658d', '#9c79db', '#d7a25a', '#e85c65', '#36b8c9']
const ICONS = {
  check: '<path d="m5 12 4 4L19 6"/>',
  general: '<circle cx="12" cy="12" r="3"/><path d="M9 3h6l1 3 3 1 2 5-2 5-3 1-1 3H9l-1-3-3-1-2-5 2-5 3-1Z"/>',
  dashboard: '<rect x="3" y="3" width="7" height="7" rx="1.5"/><rect x="14" y="3" width="7" height="7" rx="1.5"/><rect x="3" y="14" width="7" height="7" rx="1.5"/><rect x="14" y="14" width="7" height="7" rx="1.5"/>',
  routines: '<rect x="4" y="5" width="16" height="16" rx="3"/><path d="M8 3v4m8-4v4M4 11h16m-12 4h3m2 3h3"/>',
  skills: '<path d="m12 3 2.8 6.2L21 12l-6.2 2.8L12 21l-2.8-6.2L3 12l6.2-2.8L12 3Z"/>',
  connections: '<path d="m10 13 4-4m-6 7-1 1a4 4 0 0 1-6-6l4-4a4 4 0 0 1 6 0m2 1 1-1a4 4 0 0 1 6 6l-4 4a4 4 0 0 1-6 0" transform="translate(1 0)"/>',
  channels: '<rect x="3" y="5" width="18" height="14" rx="3"/><path d="m3 7 9 6 9-6"/>',
  providers: '<path d="m12 3 9 5v8l-9 5-9-5V8l9-5Zm0 0v18M3 8l9 5 9-5"/>',
  browser: '<circle cx="12" cy="12" r="9"/><path d="M3 12h18M12 3a19 19 0 0 1 0 18 19 19 0 0 1 0-18Z"/>',
  terminal: '<rect x="3" y="4" width="18" height="16" rx="3"/><path d="m7 9 3 3-3 3m6 0h4"/>',
  desktop: '<rect x="3" y="4" width="18" height="13" rx="2"/><path d="M8 21h8m-4-4v4"/>',
  memory: '<path d="M12 5c-3-2-6-2-9-1v15c3-1 6-1 9 1 3-2 6-2 9-1V4c-3-1-6-1-9 1Zm0 0v15"/>',
  details: '<circle cx="5" cy="12" r="1"/><circle cx="12" cy="12" r="1"/><circle cx="19" cy="12" r="1"/>',
  search: '<circle cx="10.5" cy="10.5" r="6.5"/><path d="m16 16 4 4"/>',
  key: '<circle cx="7.5" cy="15.5" r="4.5"/><path d="m10.7 12.3 9.3-9.3m-4 4 3 3m-6-0 2 2"/>',
  workspace: '<rect x="3" y="4" width="18" height="16" rx="3"/><path d="M15 4v16"/>',
  stop: '<rect x="6" y="6" width="12" height="12" rx="2"/>',
  clear: '<path d="M4 7h16M9 7V4h6v3m-8 0 1 13h8l1-13M10 11v6m4-6v6"/>',
  fresh: '<path d="M20 12a8 8 0 1 1-2.3-5.7M20 4v5h-5"/>',
}
const icon = name => `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${ICONS[name] || ICONS.details}</svg>`
const PAGES = { general: 'General', dashboard: 'Team', routines: 'Routines', skills: 'Plugins', connections: 'Connections & keys', channels: 'Social platforms', providers: 'Models & providers' }

const view = {
  team: null,
  work: { runs: {}, approvals: [], shots: {}, cards: [], teaching: [] },
  current: null,
  page: null,
  items: [],
  streams: {},
  panel: null,
  live: null,
  previews: {},
  teamFilter: 'all',
  search: '',
  searchIndex: 0,
  searchTotal: 0,
  searchMatch: null,
  searchTicket: 0,
  more: null,
  newer: null,
  historyLoading: false,
  conversationLoading: false,
  conversationError: null,
  restoring: false,
  routines: [],
}
const stopRequests = new Map()

async function call(method, params = {}, options = {}) {
  const response = await fetch('/api/bot', { method: 'POST', credentials: 'same-origin', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ method, params }), signal: options.signal })
  if (response.status === 401) { void goToAdminLogin(); throw new Error('登录已失效，请重新登录。') }
  const result = await response.json()
  if (!result.ok) throw new Error(result.error?.message ?? t('Request failed'))
  return result.value
}

function toast(text) {
  const el = $('#toast')
  el.textContent = text
  el.hidden = false
  clearTimeout(toast.timer)
  toast.timer = setTimeout(() => { el.hidden = true }, 3500)
}
const attempt = fn => async (...args) => { try { return await fn(...args) } catch (error) { toast(t(error.message)) } }

// Reuse unchanged rows, including loaded images, open tools and playing media.
// Compare the requested markup, not live DOM that users and preview loaders edit.
const renderedMarkup = new WeakMap()
function reconcileRows(box, html) {
  renderedMarkup.delete(box)
  const template = document.createElement('template')
  template.innerHTML = html
  const key = el => el.dataset.id ? `item:${el.dataset.id}` : el.dataset.run ? `run:${el.dataset.run}` : el.dataset.stream ? `stream:${el.dataset.stream}` : el.dataset.welcome ? `welcome:${el.dataset.welcome}` : el.dataset.open ? `chat:${el.dataset.open}` : el.dataset.page ? `page:${el.dataset.page}` : el.classList.contains('conversation-group-head') ? `group:${el.querySelector('[data-group-toggle]')?.dataset.groupToggle}` : el.classList.contains('list-label') ? `label:${el.textContent}` : null
  const previous = new Map([...box.children].map(el => [key(el), el]).filter(([id]) => id))
  let cursor = box.firstChild
  for (const fresh of [...template.content.children]) {
    const markup = fresh.outerHTML, old = previous.get(key(fresh))
    const node = old && renderedMarkup.get(old) === markup ? old : fresh
    renderedMarkup.set(node, markup)
    if (node !== cursor) {
      if (old === cursor || (cursor instanceof Element && !key(cursor) && !key(fresh))) box.replaceChild(node, cursor)
      else { if (old && old !== node) old.remove(); box.insertBefore(node, cursor) }
    }
    cursor = node.nextSibling
  }
  while (cursor) { const next = cursor.nextSibling; cursor.remove(); cursor = next }
}
function stableHtml(box, html) {
  if (renderedMarkup.get(box) === html) return false
  box.innerHTML = html
  renderedMarkup.set(box, html)
  return true
}

function skeletonHtml(kind = 'settings') {
  const line = (width, cls = '') => `<span class="skeleton-block ${cls}" style="--skeleton-width:${width}%"></span>`
  const rows = kind === 'chat'
    ? [false, true, false, true].map((user, i) => `<div class="skeleton-message ${user ? 'is-user' : ''}"><span class="skeleton-block skeleton-avatar"></span><div class="skeleton-bubble">${line(i % 2 ? 72 : 40)}${line(100)}${line(78)}</div></div>`).join('')
    : `${line(56, 'skeleton-title')}${line(92)}<div class="skeleton-stats">${[1, 2, 3].map(() => `<div>${line(42, 'skeleton-title')}${line(65)}</div>`).join('')}</div>${[1, 2, 3].map(() => `<div class="skeleton-card">${line(36)}${line(94)}${line(72)}</div>`).join('')}`
  return `<div class="loading-skeleton skeleton-${kind}" role="status" aria-label="${escape(t('Loading…'))}"><span class="visually-hidden">${t('Loading…')}</span><div aria-hidden="true">${rows}</div></div>`
}

function loadErrorHtml(retry) {
  return `<div class="load-error" role="status"><strong>${t('Content could not load')}</strong><p>${t('Check the connection and try again.')}</p><button class="ghost" ${retry}>${t('Retry')}</button></div>`
}

// ---------------------------------------------------------------------------
// Text

const TURN_STOPPED = 'the turn stopped before it finished, without saying why. Send the message again to go on.'
const short = (value, max = 90) => { const text = typeof value === 'string' ? value : JSON.stringify(value ?? ''); return text.length > max ? `${text.slice(0, max)}…` : text }
const when = ms => (ms ? new Date(ms).toLocaleString(locale(), { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' }) : '—')

// ---------------------------------------------------------------------------
// The team

const bots = () => [...(view.team?.bots ?? [])].sort((a, b) => Number(Boolean(b.pinned)) - Number(Boolean(a.pinned)))
const botName = bot => bot?.name === 'Chief' && bot.role === 'Chief of Staff' ? t('Chief') : bot?.name
const botRole = bot => t(bot.role || 'Bot')
const botDescription = bot => {
  const template = view.team?.templates?.find?.(entry => entry.description === bot.description)
  return LANG === 'zh' && template?.zh?.description ? template.zh.description : t(bot.description || '')
}
const sidebarGroups = () => view.team?.sidebarGroups ?? []
const groupOptions = value => `<option value="">${t('Ungrouped')}</option>${sidebarGroups().map(name => `<option value="${escape(name)}" ${name === value ? 'selected' : ''}>${escape(name)}</option>`).join('')}`
const rooms = () => view.team?.rooms ?? []
const botById = id => (view.team?.bots ?? []).find(bot => bot.id === id)
const colorOf = bot => bot?.color || COLORS[[...(bot?.id ?? '')].reduce((sum, c) => sum + c.charCodeAt(0), 0) % COLORS.length]
// The user as they set themselves up in Settings.
const profile = () => view.team?.prefs?.profile ?? {}
const userAvatar = () => {
  const p = profile()
  if (p.avatar) return `<img class="avatar user-avatar" src="/${escape(p.avatar)}" alt="${escape(p.name ?? t('You'))}">`
  return `<span class="avatar user-avatar">${escape((p.name || t('You')).slice(0, 1).toUpperCase())}</span>`
}
const avatar = (bot, cls = '') => {
  const seed = [...(bot?.id || bot?.name || '')].reduce((sum, c) => sum + c.charCodeAt(0), 0)
  const shapes = ['M16 2C25 2 30 8 30 17S25 30 16 30 2 25 2 16 7 2 16 2Z', 'M12 2h8a10 10 0 0 1 10 10v8a10 10 0 0 1-10 10h-8A10 10 0 0 1 2 20v-8A10 10 0 0 1 12 2Z', 'M13 2a6 6 0 0 1 6 0l9 6a6 6 0 0 1 3 5v7a6 6 0 0 1-3 5l-9 5a6 6 0 0 1-6 0l-9-5a6 6 0 0 1-3-5v-7a6 6 0 0 1 3-5Z', 'M16 1C17 7 29 13 29 20a13 13 0 0 1-26 0C3 13 13 6 16 1Z']
  return `<span class="avatar bot-avatar ${cls}" style="--bot-color:${escape(colorOf(bot))}" aria-hidden="true"><svg viewBox="0 0 32 32"><path d="${shapes[seed % shapes.length]}" fill="var(--bot-color)"/><path d="m12 12 1 5m6-6 1 5" fill="none" stroke="#fff" stroke-width="2.4" stroke-linecap="round"/></svg></span>`
}
// A Bot named in chat: its avatar and name, which open its own chat.
const botLink = (bot, cls = 'tiny', at = '') => `<button type="button" class="bot-link" data-open="${escape(bot.conversation)}" title="${escape(t('Open chat with {name}', { name: botName(bot) }))}">${avatar(bot, cls)}<span>${at}${escape(botName(bot))}</span></button>`
// "@Name" in a message becomes one chip with the @ inside it, the same rule the server
// uses: the @ starts a word (not an email) and the name ends at a space, punctuation or
// the end. A name written without @ stays plain text. Only text outside tags, code and
// links changes.
function linkBotNames(html, self) {
  const named = bots().filter(b => b.id !== self && b.conversation && b.name).sort((a, b) => b.name.length - a.name.length)
  if (!named.length) return html
  const pattern = new RegExp(`(?<![A-Za-z0-9_.+%-])[@＠](?:${named.map(b => escape(b.name).replace(/[.*+?^${}()|[\]\\]/g, '\\$&')).join('|')})(?![\\p{L}\\p{N}_-])`, 'giu')
  let skip = 0
  return html.split(/(<[^>]+>)/).map(part => {
    const tag = part.match(/^<(\/?)(code|pre|a|button)\b/i)
    if (tag) { skip += tag[1] ? -1 : 1; return part }
    if (part.startsWith('<') || skip > 0) return part
    return part.replace(pattern, found => { const bot = named.find(b => escape(b.name).toLowerCase() === found.slice(1).toLowerCase()); return bot ? botLink(bot, 'tiny', '@') : found })
  }).join('')
}
const roomAvatar = (room, cls = '') => {
  const members = room.members.map(botById).filter(Boolean).slice(0, 4)
  return `<span class="avatar room-avatar ${members.length <= 1 ? 'single' : members.length === 2 ? 'pair' : ''} ${cls}" aria-hidden="true">${members.map(bot => avatar(bot)).join('') || icon('channels')}</span>`
}
const toolLabel = name => String(name ?? '').replace(/^mcp__teambot__/, '').replace(/^mcp__browser__browser_/, 'browser ').replace(/^mcp__/, '').replace(/__/g, ' · ').replace(/_/g, ' ')
const runOf = conversation => view.work.runs?.[conversation]
// A Bot's own work: its chat and delegated tasks. Group seats run in their own lane and
// show only in that group, so replying in a group never marks the Bot itself busy.
const botRuns = bot => Object.entries(view.work.runs ?? {}).filter(([key]) => key.split('~task~')[0] === bot.conversation)
function botActivity(bot) {
  if (view.work.approvals?.some(a => a.bot === bot.id) || view.work.cards?.some(c => c.bot === bot.id)) return 'waiting'
  // Outstanding child tasks belong to the workers, not this Bot's execution slot.
  return botRuns(bot)[0]?.[1].activity ?? null
}
// The current step's subject ("cargo test", a page or file), when the Bot is in a tool.
const botDetail = bot => botActivity(bot) === 'waiting' ? '' : botRuns(bot)[0]?.[1].detail ?? ''
function hasDelegatedWork(conversation) {
  if (!conversation) return false
  const tasks = view.work.tasks ?? [], byId = new Map(tasks.map(task => [task.id, task]))
  return tasks.some(task => {
    if (!['waiting', 'queued', 'running', 'paused', 'blocked'].includes(task.status)) return false
    const seen = new Set()
    for (let current = task; current && !seen.has(current.id); current = byId.get(current.parent)) {
      if (current.source === conversation || (conversation.startsWith('room:') && current.source.startsWith(`${conversation}:`))) return true
      seen.add(current.id)
    }
    return false
  })
}
// Who is working on a conversation right now and on what: its own runs (group seats and
// task executions included) and the delegated tasks it waits for, with each worker's step.
function liveWork(conversation) {
  if (!conversation) return []
  const runs = Object.entries(view.work.runs ?? {})
  const runBot = key => { const base = key.split('~task~')[0]; return bots().find(b => b.conversation === base) ?? botById(base.split(':').at(-1)) }
  const belongs = key => { const base = key.split('~task~')[0]; return base === conversation || (conversation.startsWith('room:') && base.startsWith(`${conversation}:`)) }
  const rows = runs.filter(([key]) => belongs(key) && runBot(key)).map(([key, run]) => {
    const bot = runBot(key), waiting = view.work.approvals?.some(a => a.bot === bot.id) || view.work.cards?.some(c => c.bot === bot.id)
    const seat = conversation.startsWith('room:') && !key.includes('~task~')
    return { bot, key, text: waiting ? doingText('waiting') : seat ? t('is replying to you') : doingText(run.activity, run.detail), since: run.since, queued: run.queued }
  })
  // A group round with no member seat yet still says the group is at work.
  if (!rows.length && runs.some(([key]) => key === conversation) && conversation.startsWith('room:')) rows.push({ bot: null, key: conversation, text: doingText('thinking'), since: runOf(conversation).since })
  for (const task of conversationTasks(conversation).filter(activeTask)) {
    const bot = botById(task.target)
    if (!bot || rows.some(r => r.key.includes(`~task~${task.id}`))) continue
    const own = runs.find(([key, run]) => run.task === task.id || key.endsWith(`~task~${task.id}`))?.[1]
    const text = own ? doingText(own.activity, own.detail) : t(task.status === 'queued' ? 'Queued' : task.status)
    rows.push({ bot, key: `task:${task.id}`, text: `${t('Working on a delegated task')} · ${text}`, since: own?.since ?? task.started ?? task.created })
  }
  return rows
}
function roomStatus(room) {
  const rows = liveWork(room.conversation)
  const first = rows.find(r => r.bot) ?? rows[0]
  if (!first) return doingText('thinking')
  return `${first.bot ? `${botName(first.bot)} ` : ''}${first.text}${rows.length > 1 ? ` · ${t('{n} working', { n: rows.length })}` : ''}`
}
const roomBusy = room => Boolean(runOf(room.conversation)) || hasDelegatedWork(room.conversation) || Object.keys(view.work.runs ?? {}).some(key => key.startsWith(`room:${room.id}:`))
const currentBot = () => (view.page ? undefined : bots().find(bot => bot.conversation === view.current))
const currentRoom = () => (view.page ? undefined : rooms().find(room => room.conversation === view.current))
// A tool in use reads as its plain name in the page's language ("运行命令"), not its id.
const doingText = (doing, detail = '') => {
  const text = doingLabel(doing)
  return detail ? `${text.replace(/…$/, '')}：${detail}` : text
}
const doingLabel = doing => {
  const states = { correcting: 'Applying task correction…', 'preparing-skills': 'Preparing skills…', allocating: 'Allocating tasks…', waiting: 'waiting for you', writing: 'writing…', thinking: 'thinking…', collaborating: 'Collaborators are working…', reconnecting: 'reconnecting…', compacting: 'Summarizing earlier conversation…', 'reviewing a step': 'reviewing a step', 'generating an image': 'generating an image', 'generating a video': 'generating a video' }
  if (states[doing]) return t(states[doing])
  return TOOL_KINDS.some(([re]) => re.test(doing ?? '')) ? toolInfo({ name: doing }).label : t('Using a tool…')
}

function sidebarTime(value) {
  if (!value) return ''
  const date = new Date(value), today = new Date(); today.setHours(0, 0, 0, 0)
  if (date >= today) return date.toLocaleTimeString(locale(), { hour: '2-digit', minute: '2-digit' })
  const day = new Date(date); day.setHours(0, 0, 0, 0)
  const days = Math.round((today - day) / 86400000)
  if (days === 1) return t('Yesterday')
  return date.toLocaleDateString(locale(), days < 7 ? { weekday: 'short' } : { month: 'numeric', day: 'numeric' })
}

function newerMeta(previous, next) {
  if (!previous) return next
  const before = previous.reading?.version ?? 0, after = next.reading?.version ?? 0
  return before > after || (before === after && (previous.total ?? 0) > (next.total ?? 0)) ? previous : next
}
function mergeHistoryMeta(previous = {}, next = {}) {
  return Object.fromEntries(Object.entries(next).map(([conversation, meta]) => [conversation, newerMeta(previous[conversation], meta)]))
}

const unreadBadge = conversation => {
  const n = view.team?.history?.[conversation]?.unread ?? 0
  return n ? `<span class="unread-count" aria-label="${escape(t('{n} unread messages', { n }))}">${n > 99 ? '99+' : n}</span>` : ''
}
function renderUnread() {
  const entries = Object.values(view.team?.history ?? {})
  const other = entries.filter(m => m.unread > 0 && (m.conversation !== view.current || view.page))
  const total = other.reduce((sum, m) => sum + m.unread, 0)
  $('#more-unread').hidden = total === 0
  $('#more-unread').textContent = `↑ ${t('More unread messages')} · ${total}`
  const meta = view.team?.history?.[view.current]
  $('#unread-jump').hidden = Boolean(view.page) || !meta?.unread
  $('#unread-jump').textContent = `↓ ${t('{n} unread messages', { n: meta?.unread ?? 0 })}`
}

function renderSidebar() {
  reconcileRows($('#pages'), Object.entries(PAGES).map(([key, title]) => {
    const badge = key === 'dashboard' && (view.work.approvals?.length || view.work.cards?.length) ? `<span class="badge">${view.work.approvals.length + view.work.cards.length}</span>` : ''
    return `<button class="row page-row ${view.page === key ? 'active' : ''}" aria-current="${view.page === key ? 'page' : 'false'}" data-page="${key}" title="${escape(t(title))}"><span class="page-icon">${icon(key)}</span><span class="who"><span class="name">${escape(t(title))}${badge}</span></span></button>`
  }).join(''))
  const query = $('#team-search').value.trim().toLocaleLowerCase()
  const matches = (...fields) => !query || fields.some(s => String(s ?? '').toLocaleLowerCase().includes(query))
  const visible = [
    ...bots().map(bot => ({ entry: bot, group: bot.pinned ? 'pinned' : bot.sidebarGroup ? `group:${bot.sidebarGroup}` : 'ungrouped', pinned: Boolean(bot.pinned), doing: botActivity(bot), room: false })),
    ...rooms().map(room => ({ entry: room, group: room.sidebarGroup ? `group:${room.sidebarGroup}` : 'ungrouped', pinned: false, doing: roomBusy(room) ? 'thinking' : null, room: true })),
  ].filter(({ entry, room, doing }) => matches(entry.name, room ? '' : botName(entry), room ? '' : botRole(entry), room ? entry.preview : botDescription(entry), entry.sidebarGroup) && (view.teamFilter !== 'working' || doing))
  visible.sort((a, b) => (view.team.history?.[b.entry.conversation]?.preview?.at ?? 0) - (view.team.history?.[a.entry.conversation]?.preview?.at ?? 0))
  const p = profile()
  stableHtml($('#open-settings'), `${userAvatar()}<span>${escape(p.name || t('Settings'))}</span>${icon('details')}`)
  $('#working-count').textContent = bots().filter(bot => botActivity(bot)).length
  $('#filter-all').classList.toggle('on', view.teamFilter === 'all')
  $('#filter-working').classList.toggle('on', view.teamFilter === 'working')
  $('#filter-all').setAttribute('aria-pressed', view.teamFilter === 'all')
  $('#filter-working').setAttribute('aria-pressed', view.teamFilter === 'working')
  const sections = [
    { key: 'pinned', label: t('Pinned') },
    ...sidebarGroups().map(name => ({ key: `group:${name}`, label: name, custom: true })),
    { key: 'ungrouped', label: t('Ungrouped') },
  ]
  reconcileRows($('#bots'), sections.map(section => {
    const entries = visible.filter(item => item.group === section.key)
    if (!entries.length && (!section.custom || query || view.teamFilter !== 'all')) return ''
    const collapsed = section.custom && !query && view.teamFilter === 'all' && (view.team.prefs?.sidebarCollapsed ?? []).includes(section.label)
    const heading = section.custom ? `<div class="conversation-group-head"><button class="group-toggle" data-group-toggle="${escape(section.label)}" aria-expanded="${!collapsed}"><span class="group-chevron" aria-hidden="true">${collapsed ? '›' : '⌄'}</span><span>${escape(section.label)}</span><small>${entries.length}</small></button><button class="group-edit icon" data-group-edit="${escape(section.label)}" aria-label="${escape(t('Edit sidebar group'))}" title="${escape(t('Edit sidebar group'))}">⋯</button></div>` : `<div class="list-label">${escape(section.label)}</div>`
    return `${heading}${collapsed ? '' : entries.map(({ entry, room, doing }) => {
      const cls = doing === 'waiting' ? 'waiting' : doing ? 'busy' : ''
      const preview = view.team.history?.[entry.conversation]?.preview ?? view.previews[entry.conversation]
      return `<button class="row ${entry.conversation === view.current && !view.page ? 'active' : ''}" data-open="${escape(entry.conversation)}" aria-current="${entry.conversation === view.current && !view.page ? 'true' : 'false'}">
        ${room ? roomAvatar(entry, cls) : avatar(entry, cls)}<span class="who"><span class="row-heading"><span class="name">${escape(room ? entry.name : botName(entry))}</span>${room ? `<span class="role-tag">${t('Group chat')}</span>` : entry.role ? `<span class="role-tag">${escape(botRole(entry))}</span>` : ''}<time>${escape(sidebarTime(preview?.at))}</time>${unreadBadge(entry.conversation)}</span>
        <span class="sub ${doing ? 'is-working' : ''}">${escape(doing ? (room ? roomStatus(entry) : doingText(doing, botDetail(entry))) : preview?.text || (room ? entry.preview || t('{n} members', { n: entry.members.length }) : botDescription(entry)) || t('Start a conversation'))}</span></span></button>`
    }).join('') || `<div class="group-empty">${t('No conversations in this group')}</div>`}`
  }).join('') || `<div class="list-empty">${t('No matching conversations')}</div>`)
  renderUnread()
}

function renderHead() {
  // Settings open as a dialog over the chat; the header behind it keeps the chat's own
  // name rather than the settings page's, so a Bot never looks renamed while it is open.
  const bot = bots().find(entry => entry.conversation === view.current)
  const room = bot ? undefined : rooms().find(entry => entry.conversation === view.current)
  $('#main').classList.toggle('conversation', !view.page)
  $('#main').classList.toggle('direct-chat', Boolean(bot))
  stableHtml($('#head-avatar'), bot ? avatar(bot) : room ? roomAvatar(room) : '')
  $('#title').textContent = botName(bot) ?? room?.name ?? 'dotbot'
  if (bot) $('#subtitle').textContent = [botRole(bot), bot.main ? t('Main Bot') : '', bot.model || t('default model'), botDescription(bot)].filter(Boolean).join(' · ')
  else if (room) $('#subtitle').textContent = room.members.map(id => botName(botById(id))).filter(Boolean).join(', ') + (room.notice ? ` · ${room.notice}` : '')
  else $('#subtitle').textContent = ''
  const stopping = stopRequests.has(view.current)
  const running = conversationBusy() || stopping
  $('#act-stop').hidden = !running
  $('#act-stop').disabled = stopping
  $('#act-stop').setAttribute('aria-busy', String(stopping))
  $('#act-desktop').hidden = !bot || !(view.team?.desktop || view.team?.browser)
  $('#act-local').hidden = !bot || !view.team?.localComputer?.enabled
  $('#act-memory').hidden = !bot && !room
  $('#act-details').hidden = !bot && !room
  $('#act-clear').hidden = !bot && !room
  // Groups show their delegated work in the live status above the composer; only a Bot has an activity board.
  $('#act-activity').hidden = !bot
  $('#act-search').hidden = !bot && !room
  $('#act-panel').hidden = !bot && !room
  $('#act-panel').setAttribute('aria-expanded', String(Boolean(view.panel)))
  $('#act-panel').title = t(view.panel ? 'Close panel' : 'Open panel')
  $('#act-panel').setAttribute('aria-label', $('#act-panel').title)
  $('#act-panel').classList.toggle('on', Boolean(view.panel))
  $('#composer-hint').hidden = Boolean(view.page)
  if (view.page) { $('#chat-search').hidden = true; $('#jump-latest').hidden = true }
  $('#input').placeholder = bot ? t('Message {name}…', { name: botName(bot) }) : room ? t('Message the group…') : t('Message…')
  for (const [id, label, glyph] of [['act-search', 'Search conversation', 'search'], ['act-stop', 'Stop', 'stop'], ['act-activity', 'Team activity', 'connections'], ['act-desktop', 'Desktop & apps', 'desktop'], ['act-local', 'Local terminal', 'terminal'], ['act-memory', 'Memory', 'memory'], ['act-details', 'Details', 'details'], ['act-clear', 'Clear chat history', 'clear']]) {
    const button = $(`#${id}`)
    const displayLabel = id === 'act-stop' && stopping ? 'Stopping…' : label
    stableHtml(button, icon(glyph) + (button.closest('.conversation-menu') ? `<span>${escape(t(displayLabel))}</span>` : ''))
    button.title = t(displayLabel)
    button.setAttribute('aria-label', t(displayLabel))
  }
  $('#conversation-menu').hidden = !bot && !room
  $('#head .title').title = $('#subtitle').textContent
  $('#composer').hidden = Boolean(view.page)
  $('#attachments').hidden = Boolean(view.page) || attached.length === 0
  for (const [id, panel] of [['#act-desktop', view.team?.desktop ? 'desktop' : 'browser'], ['#act-local', 'local'], ['#act-memory', 'memory'], ['#act-details', 'details'], ['#act-activity', 'activity']]) $(id).classList.toggle('on', view.panel === panel)
  if (view.panel === 'activity' && !view.page) renderActivity()
}

// Follow the same task ancestry as Stop, including group member seats. Private
// work elsewhere never becomes part of a group's assignment board.
function conversationTasks(conversation) {
  if (!conversation) return []
  const tasks = view.work.tasks ?? [], byId = new Map(tasks.map(task => [task.id, task]))
  const belongs = key => key?.split("~task~")[0] === conversation || (conversation.startsWith('room:') && key?.startsWith(`${conversation}:`))
  return tasks.filter(task => {
    if (belongs(task.targetConversation)) return true
    const seen = new Set()
    for (let current = task; current && !seen.has(current.id); current = byId.get(current.parent)) {
      if (belongs(current.source)) return true
      seen.add(current.id)
    }
    return false
  })
}
// What the worker of a running task is doing now ("运行命令：cargo test").
const taskStep = task => { if (task.status !== 'running') return ''; const run = Object.entries(view.work.runs ?? {}).find(([key, r]) => r.task === task.id || key.endsWith(`~task~${task.id}`))?.[1]; return run ? doingText(run.activity, run.detail) : '' }
const activeTask = task => ['waiting', 'queued', 'running', 'paused', 'blocked'].includes(task.status)
const taskObjective = task => task.assignment?.objective || task.text
const taskRequirements = task => `<div class="bubble-plain">${markdown(taskObjective(task))}</div>`
function renderActivity() {
  const box = $('#activity-history')
  if (!box) return
  renderTaskCollection(box, conversationTasks(view.current), true)
}
function renderTaskCollection(box, tasks, history = false) {
  const conversation = view.current
  const signature = JSON.stringify([conversation, LANG, tasks, view.work.recovery?.map(w => [w.task,w.checkpoint]), tasks.map(task => botById(task.target))])
  if (box.dataset.signature === signature) return
  box.dataset.signature = signature
  if (!tasks.length) { box.innerHTML = `<div class="panel-empty workspace-quiet"><span class="panel-empty-icon">${icon('connections')}</span><strong>${t('No delegated work yet')}</strong><p>${t('Delegated work and results will appear here.')}</p></div>`; return }
  const working = tasks.filter(activeTask).length, completed = tasks.filter(task => task.status === 'completed').length
  const running = tasks.filter(task => task.status === 'running').length, queued = tasks.filter(task => task.status === 'queued').length
  const issues = tasks.filter(task => ['failed', 'interrupted'].includes(task.status)).length
  const shown = [...tasks.filter(activeTask).sort((a,b) => b.priority-a.priority || a.created-b.created), ...tasks.filter(task => !activeTask(task))].slice(0, 40)
  // Collapsed while work just runs: an open board takes up to a third of the height and
  // would push the message just sent out of view. Failures that need attention open it.
  const opened = new Set([...box.querySelectorAll('[data-task-row][open]')].filter(row => {
    const task = tasks.find(task => task.id === row.dataset.taskRow)
    return task && (task.status === row.dataset.taskStatus || activeTask(task))
  }).map(row => row.dataset.taskRow))
  const scroll = box.querySelector('.assignment-list')?.scrollTop ?? 0
  const focus = box.contains(document.activeElement) ? [...(document.activeElement?.attributes ?? [])].find(a=>a.name.startsWith('data-task-')) : null
  const rows = shown.map(task => {
    const bot = botById(task.target), objective = taskObjective(task)
    const result = task.result ? `<div class="assignment-result"><strong>${t('Task result')}</strong><div class="bubble-plain">${markdown(task.result)}</div></div>` : ''
    const files = (task.files ?? []).map((file, i) => fileHtml({ ...file, id: `${task.id}-artifact-${i}` }, true)).join('')
    const sender = botById(task.source.split('~task~')[0].split(':').at(-1))
    const work = (view.work.recovery ?? []).filter(w => w.task === task.id).sort((a,b) => b.created-a.created)[0]
    const dependencies = (task.dependsOn ?? []).map(id => { const prior = (view.work.tasks ?? []).find(t => t.id === id); return `<span>${escape(prior ? short(taskObjective(prior),55) : id)} · ${t(prior?.status || 'waiting')}</span>` }).join('')
    const controls = taskControls(task, work)
    return `<details class="assignment" data-task-row="${escape(task.id)}" data-task-status="${escape(task.status)}" ${opened.has(task.id) ? 'open' : ''}>
      <summary>${avatar(bot)}<span class="assignment-label"><strong>${escape(botName(bot) || t('Deleted Bot'))}<small>${escape(t('Assigned by {name}', { name: botName(sender) || t('Bot') }))}</small></strong><span>${escape(short(objective, 130))}</span></span><span class="assignment-status ${escape(task.status)}" title="${escape(taskStep(task))}">${t(task.status)}${taskStep(task) ? ` · ${escape(short(taskStep(task), 40))}` : ''}</span></summary>
      <div class="assignment-body"><div class="assignment-meta">第 ${task.attempt || 1} 次执行 · ${escape(task.id)} · ${escape((task.path ?? []).map(id => botName(botById(id)) || t('Deleted Bot')).join(' → '))}</div>
      ${dependencies ? `<div class="task-dependencies"><strong>前置步骤</strong>${dependencies}</div>` : ''}${work?.checkpoint ? `<details class="task-checkpoint"><summary>当前检查点</summary><div class="bubble-plain">${markdown(work.checkpoint)}</div></details>` : ''}${taskRequirements(task)}${(task.corrections ?? []).map(c => `<div class="assignment-correction"><strong>${t('Task correction')}</strong><div class="bubble-plain">${markdown(c.text)}</div></div>`).join('')}${result}${files ? `<div class="assignment-files">${files}</div>` : ''}<div class="assignment-actions"><button class="ghost" data-open="bot:${escape(task.target)}">${t('View Bot')}</button>${controls}</div></div></details>`
  }).join('')
  const counts = [running ? t('{n} running', { n: running }) : '', queued ? t('{n} queued', { n: queued }) : '', history ? t('{n} completed', { n: completed }) : '', issues ? t('{n} need attention', { n: issues }) : ''].filter(Boolean).join(' · ')
  box.innerHTML = `<p class="activity-summary">${counts}</p><div class="assignment-list">${rows}</div>`
  box.querySelector('.assignment-list').scrollTop = scroll
  if (focus) box.querySelector(`[${focus.name}="${CSS.escape(focus.value)}"]`)?.focus()
}

// ---------------------------------------------------------------------------
// The conversation

// What a tool call shows: an icon, a plain name and the one input that says what it did.
const TOOL_KINDS = [
  [/^Bash$|(?:desktop|local)_run$/, '⌘', 'Ran a command', i => i.command],
  [/^Read$/, '📄', 'Read a file', i => i.file_path],
  [/^(Write|NotebookEdit)$/, '✎', 'Wrote a file', i => i.file_path ?? i.notebook_path],
  [/^(Edit|MultiEdit)$/, '✎', 'Edited a file', i => i.file_path],
  [/^(Grep|Glob)$/, '⌕', 'Searched files', i => i.pattern],
  [/^WebSearch$/, '⌕', 'Searched the web', i => i.query],
  [/^WebFetch$/, '🌐', 'Read a web page', i => i.url],
  [/^(Task|Agent)$/, '◇', 'Ran a helper', i => i.description],
  [/^TodoWrite$/, '☑', 'Updated the plan', () => ''],
  [/^ToolSearch$/, '⌕', 'Looked up tools', () => ''],
  [/(?:^|__)desktop_browser$/, '🌐', 'Opened desktop browser', i => i.url],
  [/browser_navigate$/, '🌐', 'Opened a page', i => i.url],
  [/browser_navigate_back$/, '🌐', 'Went back', () => ''],
  [/browser_(snapshot|take_screenshot)$/, '👁', 'Looked at the page', () => ''],
  [/browser_(click|mouse_click_xy)$/, '🖱', 'Clicked', i => i.element ?? (i.x != null ? `${i.x}, ${i.y}` : '')],
  [/browser_(type|fill_form|press_key|select_option)$/, '⌨', 'Typed in the page', i => i.text ?? i.key ?? i.element ?? ''],
  [/browser_/, '🌐', 'Used the browser', () => ''],
  [/(?:desktop|local)_screenshot$/, '🖥', 'Looked at the desktop', () => ''],
  [/(?:desktop|local)_(click|drag)$/, '🖱', 'Clicked on the desktop', i => (i.x != null ? `${i.x}, ${i.y}` : '')],
  [/(?:desktop|local)_(type|key)$/, '⌨', 'Typed on the desktop', i => i.text ?? i.keys],
  [/(?:desktop|local)_open$/, '🖥', 'Opened on the desktop', i => i.target],
  [/(?:desktop|local)_scroll$/, '🖥', 'Scrolled the desktop', () => ''],
  [/(?:^|__)(?:local_)?send_file$/, '📎', 'Sent a file', i => i.path],
  [/(?:^|__)message_bot$/, '✉', 'Messaged a Bot', i => i.bot],
  [/(?:^|__)post_to_group$/, '✉', 'Posted to a group', i => i.group],
  [/(?:^|__)(remember|forget|recall)$/, '🧠', 'Used memory', i => i.text ?? i.query ?? i.entry],
  [/(?:^|__)ask_user$/, '?', 'Asked you', i => i.question],
  [/(?:^|__)request_secret$/, '🔑', 'Asked for a key', i => i.name],
  [/(?:^|__)project_merge$/, '⇄', '合并项目改动', i => i.root],
  [/(?:^|__)task_checkpoint$/, '✓', 'Saved progress', () => ''],
  [/(?:^|__)list_accounts$/, '◇', 'Checked connected accounts', () => ''],
  [/(?:^|__)account_request$/, '◇', 'Used a connected account', () => ''],
  [/(?:^|__)local_status$/, '⌘', 'Checked local terminal', () => ''],
  [/(?:^|__)local_(receive_file|release)$/, '⌘', 'Used local terminal', () => ''],
]
function toolInfo(item) {
  const input = item.input ?? {}
  const kind = TOOL_KINDS.find(([re]) => re.test(item.name ?? ''))
  if (!kind) return { icon: '⚙', label: toolLabel(item.name), arg: short(input, 80) }
  const arg = kind[3](input)
  return { icon: kind[1], label: t(kind[2]), arg: arg == null ? '' : short(String(arg), 80) }
}
// Team actions read like Grok's: a card that says what happened ("发送消息给 测试bot",
// "创建 PPT创作") with what was sent, instead of a tool call. The plan (TodoWrite) is a checklist.
const botByName = name => (view.team?.bots ?? []).find(b => b.name.toLowerCase() === String(name ?? '').trim().toLowerCase())
const roomByName = name => (view.team?.rooms ?? []).find(r => r.name.toLowerCase() === String(name ?? '').trim().toLowerCase())
const HIDDEN_TOOLS = /^(ToolSearch|mcp__teambot__list_bots|mcp__teambot__list_routines|mcp__teambot__list_secrets)$/
function actionState(item) {
  if (!item.done) return conversationBusy() ? `<span class="act-state run">${t('in progress')}</span>` : `<span class="act-state err">${t('stopped')}</span>`
  return item.error ? `<span class="act-state err">${t('failed')}</span>` : ''
}
function actionCard(item, { icon, title, who, body, meta }) {
  const failure = item.done && item.error && item.result ? `<div class="act-error">${escape(t(item.result))}</div>` : ''
  return `<div class="action" data-id="${escape(item.id)}"><div class="act-head"><span class="act-icon">${icon}</span><span class="act-title">${title}</span>${who ?? ''}${actionState(item)}</div>${body ? `<div class="act-body">${body}</div>` : ''}${meta ? `<div class="act-meta">${meta}</div>` : ''}${failure}</div>`
}
const chip = (bot, name) => bot?.conversation ? `<span class="act-who">${botLink(bot)}</span>` : `<span class="act-who">${avatar(bot ?? { name, id: name }, 'tiny')}${escape(botName(bot) ?? name ?? '')}</span>`
// A message to another Bot mirrors the handoff it becomes in the recipient's chat ("Message from ▢ Name"):
// a one-line "Message to ▢ Name" that opens to the text sent.
// The recipient by id (kept through renames), else by the name the Bot wrote.
const handoffBot = item => botById(item.target) ?? botByName(item.input?.bot)
const sentToBot = item => item.kind === 'tool' && /__message_bot$/.test(item.name ?? '')
function assignmentHandoff(item, bot, name) {
  const i = item.input ?? {}
  const failure = item.done && item.error && item.result ? `<div class="act-error">${escape(t(item.result))}</div>` : ''
  const who = bot?.conversation ? botLink(bot) : `${bot ? avatar(bot, 'tiny') : ''}<span>${escape(botName(bot) ?? name ?? '')}</span>`
  return `<details class="handoff outgoing" data-id="${escape(item.id)}"><summary><span>${t('Message to')}</span>${who}${actionState(item)}</summary><div class="bubble">${markdown(i.text ?? '')}${i.reply_to === 'user' ? `<p class="act-meta">${t('replies to you directly')}</p>` : ''}</div></details>${failure}`
}
function actionHtml(item) {
  const name = item.name ?? '', i = item.input ?? {}
  if (/__message_bot$/.test(name)) return assignmentHandoff(item, handoffBot(item), i.bot)
  if (/__post_to_group$/.test(name)) return actionCard(item, { icon: '↗', title: t('Posted to group'), who: `<span class="act-who"># ${escape(roomByName(i.group)?.name ?? i.group ?? '')}</span>`, body: markdown(i.text ?? '') })
  if (/__create_bot$/.test(name)) return actionCard(item, { icon: '＋', title: t('Created Bot'), who: chip(botByName(i.name), i.name), body: [i.role ? `<div><b>${t('Role')}</b> ${escape(i.role)}</div>` : '', i.description ? `<div><b>${t('For')}</b> ${escape(i.description)}</div>` : '', i.instructions ? `<details><summary>${t('Instructions')}</summary>${markdown(i.instructions)}</details>` : ''].join('') })
  if (/__create_group$/.test(name)) return actionCard(item, { icon: '＋', title: t('Created group chat'), who: `<span class="act-who"># ${escape(i.name ?? '')}</span>`, body: (i.members ?? []).map(m => chip(botByName(m), m)).join('') })
  if (/__update_bot$/.test(name)) return actionCard(item, { icon: '✎', title: t('Updated'), who: chip(botByName(i.bot) ?? botById(item.bot), i.bot || botById(item.bot)?.name), body: ['name', 'role', 'description', 'model'].filter(k => i[k]).map(k => `<div><b>${t({ name: 'Name', role: 'Role', description: 'For', model: 'Model' }[k])}</b> ${escape(i[k])}</div>`).join('') })
  if (/__create_routine$/.test(name)) return actionCard(item, { icon: '↻', title: t('Created routine'), who: `<span class="act-who">${escape(i.title ?? '')}</span>`, body: markdown(i.prompt ?? ''), meta: i.when ? escape(describeWhen(i.when)) : '' })
  if (/__ask_user$/.test(name)) return actionCard(item, { icon: '?', title: t('Asked you'), body: markdown(i.question ?? '') })
  if (/^TodoWrite$/.test(name)) {
    const todos = i.todos ?? []
    const done = todos.filter(x => x.status === 'completed').length
    const rows = todos.map(x => `<li class="plan-${escape(x.status)}"><span class="plan-mark">${x.status === 'completed' ? '✓' : x.status === 'in_progress' ? '◐' : ''}</span>${escape(x.status === 'in_progress' ? x.activeForm || x.content : x.content)}</li>`).join('')
    return actionCard(item, { icon: '☰', title: t('Plan'), who: `<span class="act-count">${done}/${todos.length}</span>`, body: `<ol class="plan">${rows}</ol>` })
  }
  return null
}

function toolHtml(item, bot) {
  const { icon, label, arg } = toolInfo(item)
  const stale = !item.done && !conversationBusy()
  const state = stale ? `<span class="state err">${t('stopped')}</span>` : !item.done ? `<span class="state run">${t('running')}</span>` : item.error ? `<span class="state err">${t('failed')}</span>` : `<span class="state ok">✓</span>`
  const shots = (item.images ?? []).map(src => `<img class="shot" src="/${escape(src)}" alt="${t('Screenshot')}" loading="lazy">`).join('')
  const who = currentRoom() && bot ? `${escape(bot.name)} · ` : ''
  const result = item.result ? `<div class="tool-sec">${t('Result')}</div><pre class="${item.error ? 'err' : ''}">${escape(t(item.result))}</pre>` : ''
  return `<details class="tool ${item.error ? 'failed' : ''}" data-id="${escape(item.id)}"><summary><span class="ticon">${icon}</span><span class="tname">${who}${escape(label)}</span><span class="targ" title="${escape(arg)}">${escape(arg)}</span>${state}</summary>
      <div class="body"><div class="tool-sec">${LANG === 'zh' ? t('Details') : escape(toolLabel(item.name))}</div><pre>${escape(JSON.stringify(item.input, null, 2))}</pre>${result}</div>${shots}</details>`
}
// Three or more steps in a row fold into one line, the newest step kept in sight.
function itemsHtml(all) {
  // Also hide traces saved by older versions.
  // Messages sent to other Bots stay, as handoff lines.
  const items = all.filter(x => x.kind !== 'tool' || sentToBot(x))
  const out = []
  for (let i = 0; i < items.length;) {
    if (items[i].kind !== 'tool' || actionHtml(items[i])) { out.push(itemHtml(items[i])); i++; continue }
    let j = i
    while (j < items.length && items[j].kind === 'tool' && !actionHtml(items[j])) j++
    const run = items.slice(i, j)
    if (run.length < 3) out.push(...run.map(itemHtml))
    else {
      const failed = run.filter(x => x.error).length
      const busy = run.some(x => !x.done)
      out.push(`<details class="tool-run" data-run="${escape(run[0].id)}"><summary>${t('{n} steps', { n: run.length - 1 })}${failed ? ` · <span class="state err">${t('{n} failed', { n: failed })}</span>` : ''}${busy ? ` · <span class="state run">${t('running')}</span>` : ''}</summary>${run.slice(0, -1).map(itemHtml).join('')}</details>${itemHtml(run.at(-1))}`)
    }
    i = j
  }
  return out.join('')
}

// The user's message, with Edit under it, and Stop under the newest one while the Bot works.
// Edit sends the changed text as a new message (stopping the work on the old one first): a
// Bot's conversation cannot be rewound, so the original stays above it.
const conversationBusy = (conversation = view.current) => Object.entries(view.work.runs ?? {}).some(([key]) => key.split('~task~')[0] === conversation) || hasDelegatedWork(conversation) || Boolean(conversation && (runOf(conversation) || (conversation.startsWith('room:') && Object.keys(view.work.runs ?? {}).some(key => key.startsWith(`${conversation}:`))))) || Object.values(view.streams ?? {}).some(s => s.conversation === conversation && s.text)

function stopConversation(conversation = view.current) {
  if (!conversation) return Promise.resolve(false)
  if (stopRequests.has(conversation)) return stopRequests.get(conversation)
  const request = call('chat.stop', { conversation }).finally(() => {
    stopRequests.delete(conversation)
    if (view.current === conversation && !view.page) { renderHead(); renderMessages({ keepScroll: true }) }
  })
  stopRequests.set(conversation, request)
  if (view.current === conversation && !view.page) { renderHead(); renderMessages({ keepScroll: true }) }
  return request
}
// Whether this user message is being answered now, waiting in the queue, or neither.
// Group chats are answered as rounds, so they keep the whole-conversation Stop.
function itemWork(id, conversation = view.current) {
  const run = runOf(conversation)
  if (run?.items?.includes(id)) return 'running'
  if (run?.queuedItems?.includes(id)) return 'queued'
  return null
}
const itemStops = new Map()
function stopItem(id, conversation = view.current) {
  const key = `${conversation}\n${id}`
  if (itemStops.has(key)) return itemStops.get(key)
  const request = call('chat.stop_item', { conversation, item: id }).finally(() => {
    itemStops.delete(key)
    if (view.current === conversation && !view.page) { renderHead(); renderMessages({ keepScroll: true }) }
  })
  itemStops.set(key, request)
  if (view.current === conversation && !view.page) renderMessages({ keepScroll: true })
  return request
}
function userHtml(item) {
  const attachedFiles = (item.files ?? []).map((file, i) => `<div class="user-file">${fileHtml({ ...file, id: `${item.id}-file-${i}` }, true)}</div>`).join('')
  const pictures = (item.images ?? []).filter(src => !(item.files ?? []).some(f => f.src === src)).map(src => `<img class="shot" src="/${escape(src)}" alt="${t('Image')}">`).join('')
  const editing = view.editing?.id === item.id
  const sending = editing && view.editing.sending
  const room = view.current?.startsWith('room:')
  const work = room ? null : itemWork(item.id)
  const stopping = room ? stopRequests.has(view.current) : itemStops.has(`${view.current}\n${item.id}`)
  const body = editing
    ? `<div class="edit-bubble" aria-busy="${Boolean(sending)}"><textarea id="edit-box" aria-label="${t('Edit message')}" rows="2" ${sending ? 'disabled' : ''}>${escape(view.editing.draft)}</textarea><div class="edit-foot"><span class="muted">${t(work ? 'Enter to stop this message and run the edited one · Esc to cancel' : 'Enter to send as a new message · Shift+Enter for a new line · Esc to cancel')}</span><button class="ghost" data-edit-cancel ${sending ? 'disabled' : ''}>${t('Cancel')}</button><button class="primary" data-edit-send="${escape(item.id)}" ${sending ? 'disabled' : ''}>${t(sending ? 'Sending…' : 'Send')}</button></div></div>`
    : `<div class="bubble">${pictures}${attachedFiles}${['(image)', '(attachments)'].includes(item.text) && (item.files?.length || item.images?.length) ? '' : markdown(item.text)}</div>`
  const newest = [...view.items].reverse().find(i => i.kind === 'user')?.id === item.id
  const pencil = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M12 20h9"/><path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4z"/></svg>'
  const stopLabel = t(stopping ? 'Stopping…' : work === 'queued' ? 'Cancel' : 'Stop')
  const actions = editing ? '' : `<div class="msg-actions">${item.text && !['(image)', '(attachments)'].includes(item.text) ? `<button class="msg-btn icon-only edit-btn" data-edit="${escape(item.id)}" aria-label="${t('Edit message')}" title="${t('Edit message')}">${pencil}</button>` : ''}${(room ? newest && (conversationBusy() || stopping) : work || stopping) ? `<button class="msg-btn icon-only stop-btn" ${room ? 'data-stop-run' : `data-stop-item="${escape(item.id)}"`} ${stopping ? 'disabled aria-busy="true"' : ''} aria-label="${stopLabel}" title="${stopLabel}"><i aria-hidden="true"></i></button>` : ''}${work === 'queued' ? `<span class="msg-queued muted">${t('Queued')}</span>` : ''}</div>`
  const via = item.via ? `<div class="via muted">${escape(t('via {where}', { where: item.via }))}</div>` : ''
  const correction = item.correction ? `<div class="via muted">${escape(t('Correction for {name}', { name: botName(botById(item.targetBot)) || t('Bot') }))}</div>`
    : item.steered ? `<div class="via muted">${escape(t('Sent to {name} while it works', { name: botName(botById(item.targetBot)) || t('Bot') }))}</div>` : ''
  return `<div class="msg user" data-id="${escape(item.id)}"><div class="user-col">${via}${correction}${body}${actions}</div>${userAvatar()}</div>`
}

function itemHtml(item) {
  const bot = botById(item.bot)
  if (item.kind === 'delivery') return ''
  if (item.kind === 'user') return userHtml(item)
  if (item.kind === 'notice' && item.challenge) return `<div class="notice error challenge" data-id="${escape(item.id)}">🛡 ${escape(t('{name} hit a human check. Complete it yourself in its browser panel; it will not try to get past it.', { name: botById(item.challenge)?.name ?? t('The Bot') }))} <button class="ghost" data-open-browser="${escape(item.challenge)}">${t('Open the browser')}</button></div>`
  if (item.kind === 'notice' && /^(Auto-approved |Memory: )/.test(item.text ?? '')) return ''
  if (item.kind === 'notice') {
    // Failures saved before the reason was always written read "Name: " and nothing else.
    const text = item.error && /:\s*$/.test(item.text ?? '') ? `${item.text.trim()} ${TURN_STOPPED}` : item.text
    const streamError = item.streamError || (item.error && /server error mid-response|stream terminated|premature close/i.test(text))
    if (streamError) {
      const message = item.usedTools ? 'The model connection was interrupted after tools ran. Check completed actions before continuing.' : item.retryCount ? 'The model connection is still unavailable after automatic retries. Try again shortly.' : 'The model service interrupted the response. Try again shortly.'
      const partial = item.partial ? `<details class="partial-response" open><summary>${t('Incomplete response')}</summary><div class="bubble">${markdown(item.partial)}</div></details>` : ''
      return `<div class="notice error stream-error" data-id="${escape(item.id)}"><strong>${escape(botName(bot) || String(text || '').split(':')[0])}</strong><span>${t(message)}</span>${partial}<details><summary>${t('Details')}</summary><pre>${escape(text)}</pre></details></div>`
    }
    const modelError = item.modelError || (item.error && /model[\s\S]*(?:not supported|unsupported|not found|does not exist|not available)/i.test(text))
    if (modelError) return `<div class="notice error model-error" data-id="${escape(item.id)}"><strong>${escape(t('Model unavailable'))}</strong><span>${escape(t('The model is unavailable. Choose a supported text model in settings, then send your message again.'))}</span><div class="model-error-foot"><button class="ghost" data-model-settings>${t('Models & providers')}</button><details><summary>${t('Details')}</summary><pre>${escape(text)}</pre></details></div></div>`
    return `<div class="notice ${item.error ? 'error' : ''}" data-id="${escape(item.id)}">${escape(t(text))}</div>`
  }
  if (item.kind === 'incoming') {
    const heading = (item.text || '').match(/^\[(Reply from|Message from) ([^\]]+)\]\n?/)
    if (heading) {
      // Direct requests add " · the user sees your reply here" after the name; older items have no sender id.
      const written = heading[2].split(' · ')[0].trim(), sender = botById(item.from) || botByName(written)
      const name = sender ? botName(sender) : written
      return `<details class="handoff" data-id="${escape(item.id)}"><summary><span>${t(heading[1] === 'Reply from' ? 'Reply from' : 'Message from')}</span>${sender?.conversation ? botLink(sender) : `${sender ? avatar(sender, 'tiny') : ''}<span>${escape(name)}</span>`}</summary><div class="bubble">${markdown(item.text.slice(heading[0].length))}</div></details>`
    }
    return `<div class="msg incoming" data-id="${escape(item.id)}"><div class="bubble">${markdown(item.text)}</div></div>`
  }
  if (sentToBot(item)) return assignmentHandoff(item, handoffBot(item), item.input?.bot)
  if (item.kind === 'tool') return ''
  if (item.kind === 'file') {
    if (view.items.some(i=>i.kind==='delivery' && (i.files??[]).some(f=>f.src===item.src))) return ''
    const speaker = bot ?? { name: t('Bot'), id: item.bot }
    return `<div class="msg bot" data-id="${escape(item.id)}">${avatar(speaker)}<div><div class="speaker">${escape(speaker.name)}</div><div class="bubble file-card">${fileHtml(item)}</div></div></div>`
  }
  const speaker = bot ?? { name: item.who ?? t('Bot'), id: item.who }
  return `<div class="msg bot${item.retracted ? ' retracted' : ''}" data-id="${escape(item.id)}">${avatar(speaker)}<div><div class="speaker">${escape(speaker.name)}</div>${item.retracted ? `<details class="retracted-note"><summary>${t('Reply retracted')}</summary><div class="bubble">${linkBotNames(markdown(item.text), speaker.id)}</div></details>` : `<div class="bubble">${linkBotNames(markdown(item.text), speaker.id)}</div>`}${replyActions(item)}</div></div>${reasonRow(item)}`
}
const THUMB = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M7 11v9H4v-9z"/><path d="M7 11l4-8a2 2 0 0 1 2 2v4h6a2 2 0 0 1 2 2l-1 7a2 2 0 0 1-2 2H7"/></svg>'
const UNDO = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M9 14L4 9l5-5"/><path d="M4 9h10a6 6 0 0 1 0 12h-3"/></svg>'
// Rate a Bot's reply (right / wrong) or take it back; the Bot keeps the verdict in its memory.
// After a thumbs-down, an optional "what was wrong" row: a tap on a reason (or a typed one)
// goes into the lesson the Bot keeps, so it knows what to avoid next time.
const BAD_REASONS = ['Inaccurate', 'Too long', 'Did not follow my request', 'Off topic']
let reasonFor = null
function replyActions(item) {
  if (!item.text || item.retracted) return item.retracted ? `<div class="msg-actions reply-actions"><button class="msg-btn icon-only" data-rate="" data-rate-item="${escape(item.id)}" aria-label="${t('Restore reply')}" title="${t('Restore reply')}">${UNDO}</button></div>` : ''
  const on = item.feedback
  const btn = (kind, label, svg) => `<button class="msg-btn icon-only rate-${kind} ${on === kind ? 'on' : ''}" data-rate="${on === kind ? '' : kind}" data-rate-item="${escape(item.id)}" aria-pressed="${on === kind}" aria-label="${label}" title="${label}">${svg}</button>`
  const given = on === 'bad' && item.feedbackReason ? `<button class="rate-given" data-reason-open="${escape(item.id)}" title="${t('Change the reason')}">${escape(t('Feedback: {reason}', { reason: item.feedbackReason }))}</button>` : ''
  return `<div class="msg-actions reply-actions${on ? ' rated' : ''}">${btn('good', t('Good reply'), THUMB)}${btn('bad', t('Bad reply'), THUMB)}<button class="msg-btn icon-only" data-retract="${escape(item.id)}" aria-label="${t('Retract reply')}" title="${t('Retract reply')}">${UNDO}</button>${given}</div>`
}
// Its own row under the message, so it never widens the reply bubble.
const reasonRow = item => item.feedback === 'bad' && reasonFor === item.id && !item.retracted ? `<section class="rate-reason" data-reason-box="${escape(item.id)}" aria-label="${t('What was wrong?')}">
  <div class="rate-reason-head"><strong>${t('What was wrong?')}</strong><button type="button" class="rate-reason-skip" data-reason-skip>${t('Skip')}</button></div>
  <div class="rate-reason-options">${BAD_REASONS.map(r => `<button type="button" class="chip" data-reason="${escape(t(r))}" data-reason-item="${escape(item.id)}" aria-pressed="${item.feedbackReason === t(r)}">${t(r)}</button>`).join('')}</div>
  <div class="rate-reason-compose"><input data-reason-input="${escape(item.id)}" aria-label="${t('Feedback details')}" placeholder="${t('Add a little more detail (optional)')}" maxlength="120"><button type="button" class="rate-reason-send" data-reason-submit="${escape(item.id)}">${t('Submit feedback')}</button></div>
</section>` : ''
const rateReply = (item, rating, extra = {}) => call('chat.rate', { conversation: view.current, item, rating, ...extra })
const submitReason = attempt(async item => {
  const input = document.querySelector(`[data-reason-input="${CSS.escape(item)}"]`)
  const reason = input?.value.trim()
  if (!reason) { input?.focus(); return }
  await rateReply(item, 'bad', { reason })
  reasonFor = null
  renderMessages({ keepScroll: true })
  toast(t('Thanks, the Bot will remember why'))
})

// A file a Bot sent, shown the way it is used: images and video in place, audio playable,
// Other documents open in the shared file viewer; small text files also render inline.
function fileHtml(item, compact = false) {
  const src = escape(FilePreview.url(item))
  const type = FilePreview.kind(item)
  const keep = `data-keep="${escape(item.id)}"`
  let view = ''
  if (type === 'image') view = `<a href="${src}" target="_blank" rel="noopener"><img class="file-view" src="${src}" alt="${escape(item.name)}" loading="lazy"></a>`
  else if (type === 'video') view = `<video ${keep} class="file-view" src="${src}" controls preload="metadata"></video>`
  else if (type === 'audio') view = `<audio ${keep} class="file-audio" src="${src}" controls preload="metadata"></audio>`
  else if (!compact) view = FilePreview.inlineHtml(item)
  const previewLink = FilePreview.button(item)
  const size = item.size >= 1e6 ? `${(item.size / 1e6).toFixed(1)} MB` : `${Math.max(1, Math.round((item.size ?? 0) / 1e3))} KB`
  return `${item.caption ? `<div>${markdown(item.caption)}</div>` : ''}${view}<div class="file-meta"><span>📄 ${escape(item.name)} · ${size}</span>${previewLink}<a href="${src}" download="${escape(item.name)}">${t('Download')}</a></div>`
}
function fillTextPreviews(root = document) {
  void FilePreview.fillInline(root)
}

function streamHtml() {
  return Object.entries(view.streams).filter(([, s]) => s.conversation === view.current && s.text).map(([key, s]) => {
    const bot = botById(s.bot) ?? { name: t('Bot'), id: key }
    return `<div class="msg bot streaming" data-stream="${escape(key)}">${avatar(bot)}<div><div class="speaker">${escape(bot.name)}</div><div class="bubble">${linkBotNames(markdown(s.text), bot.id)}</div></div></div>`
  }).join('')
}

function renderMessages({ keepScroll = false } = {}) {
  if (view.page) { renderPage({ preserveDraft: true }); return }
  const box = $('#messages')
  const atEnd = box.scrollHeight - box.scrollTop - box.clientHeight < 80
  $('#chat-retry').hidden = !view.conversationError || !view.items.length
  box.setAttribute('aria-busy', String(view.conversationLoading))
  if (view.conversationLoading && !view.items.length) { stableHtml(box, skeletonHtml('chat')); return }
  if (view.conversationError && !view.items.length) { stableHtml(box, loadErrorHtml('data-chat-retry')); return }
  if (!view.current) {
    stableHtml(box, `<div class="empty"><h2>${t('Your Bot team')}</h2><p>${t('Pick a Bot or a group chat on the left.')}</p></div>`)
    return
  }
  const open = new Set([...box.querySelectorAll('details.tool[open], details.handoff[open]')].map(el => el.dataset.id))
  const openRuns = new Set([...box.querySelectorAll('details.tool-run[open]')].map(el => el.dataset.run))
  // Players and previews move over as they are, so a video keeps playing while text streams in.
  const kept = new Map([...box.querySelectorAll('[data-keep]')].map(el => [el.dataset.keep, el]))
  const editFocus = document.activeElement?.id === 'edit-box' ? [document.activeElement.selectionStart, document.activeElement.selectionEnd] : null
  const html = (view.more != null ? `<button class="history-load ghost" data-load-older>${t(view.historyLoading ? 'Loading history…' : 'Load earlier messages')}</button>` : '') + itemsHtml(view.items) + (view.newer == null ? streamHtml() : `<button class="history-load ghost" data-load-newer>${t('Load newer messages')}</button>`)
  reconcileRows(box, html || (currentBot() ? `<div class="chat-welcome" data-welcome="${escape(view.current)}"><strong>${t('What kind of help would you like first?')}</strong><p>${t('Pick one to get started. You can always change it later.')}</p><div class="starter-prompts">${[['Code & projects', 'Help me work on a coding project.'], ['Research & organize', 'Help me research a topic and compare the best sources.'], ['Everyday tasks', 'Help me prioritize my tasks and plan my day.'], ['Just chat', 'Let’s talk and explore what you can help me with.']].map(([label, draft], i) => `<button data-draft="${escape(t(draft))}"><span>${['A', 'B', 'C', 'D'][i]}</span>${t(label)}</button>`).join('')}</div><input id="starter-answer" aria-label="${t('Your own answer')}" placeholder="${t('Your own answer')}"></div>` : `<div class="empty"><p>${t('Describe your goal. The coordinator will assign suitable members and integrate their results. You can also call @Name or @all.')}</p></div>`))
  if ($('#starter-answer')) $('#starter-answer').onkeydown = event => {
    if (event.key === 'Enter' && !event.isComposing && event.target.value.trim()) { $('#input').value = event.target.value; $('#input').focus() }
  }
  for (const id of open) box.querySelector(`details[data-id="${CSS.escape(id)}"]`)?.setAttribute('open', '')
  for (const id of openRuns) box.querySelector(`details.tool-run[data-run="${CSS.escape(id)}"]`)?.setAttribute('open', '')
  for (const el of box.querySelectorAll('[data-keep]')) { const old = kept.get(el.dataset.keep); if (old && old !== el) el.replaceWith(old) }
  // Streaming redraws the list; an open edit keeps its text and cursor.
  const editBox = $('#edit-box')
  if (editBox) {
    const fit = () => { editBox.style.height = 'auto'; editBox.style.height = `${Math.min(260, editBox.scrollHeight)}px` }
    fit()
    editBox.oninput = () => { view.editing.draft = editBox.value; fit() }
    editBox.onkeydown = (event) => { if (event.key === 'Enter' && !event.shiftKey && !event.isComposing) { event.preventDefault(); box.querySelector('[data-edit-send]')?.click() } else if (event.key === 'Escape') { view.editing = null; renderMessages({ keepScroll: true }) } }
    if (editFocus) { editBox.focus(); editBox.setSelectionRange(...editFocus) }
  }
  fillTextPreviews(box)
  highlightSearch()
  if ((!keepScroll || atEnd) && view.newer == null && !view.restoring) box.scrollTop = box.scrollHeight
  updateScrollAffordance()
  scheduleReading()
}
let messageFrame
function queueMessages() {
  if (messageFrame) return
  messageFrame = requestAnimationFrame(() => { messageFrame = null; if (!view.page) renderMessages({ keepScroll: true }) })
}

function highlightSearch() {
  const query = $('#chat-search-input').value.trim().toLocaleLowerCase()
  for (const message of $('#messages').querySelectorAll('.msg, .handoff')) {
    message.classList.toggle('search-match', Boolean(query && message.textContent.toLocaleLowerCase().includes(query)))
    message.classList.toggle('search-current', Boolean(query && message.dataset.id === view.searchMatch))
  }
  $('#search-count').textContent = query ? `${view.searchTotal ? view.searchIndex + 1 : 0} / ${view.searchTotal}` : ''
  $('#search-prev').disabled = $('#search-next').disabled = !view.searchTotal
}
const searchConversation = attempt(async (scroll = true) => {
  const query = $('#chat-search-input').value.trim()
  const conversation = view.current, ticket = ++view.searchTicket
  if (!query || !conversation) { view.searchTotal = 0; view.searchMatch = null; highlightSearch(); return }
  let index = view.searchTotal ? ((view.searchIndex % view.searchTotal) + view.searchTotal) % view.searchTotal : 0
  let result = await call('chat.search', { conversation, query, offset: index })
  if (ticket !== view.searchTicket || conversation !== view.current) return
  if (result.total && index >= result.total) { index = 0; result = await call('chat.search', { conversation, query, offset: 0 }) }
  if (ticket !== view.searchTicket || conversation !== view.current) return
  view.searchTotal = result.total; view.searchIndex = index; view.searchMatch = result.matches[0]?.id ?? null
  if (scroll && view.searchMatch && !view.items.some(i => i.id === view.searchMatch)) {
    const page = await call('chat.page', { conversation, anchor: view.searchMatch })
    if (ticket !== view.searchTicket || conversation !== view.current) return
    view.items = page.items; view.more = page.more; view.newer = page.newer
    view.restoring = true; renderMessages({ keepScroll: true }); view.restoring = false
  }
  highlightSearch()
  const target = view.searchMatch && $('#messages').querySelector(`[data-id="${CSS.escape(view.searchMatch)}"]`)
  if (target && scroll) { if (target.matches('.handoff')) target.open = true; target.scrollIntoView({ block: 'center' }); scheduleReading() }
})

function mergeMessages(before, after) {
  const items = new Map(before.map(i => [i.id, i])); for (const item of after) items.set(item.id, item)
  return [...items.values()].sort((a, b) => (a.time ?? 0) - (b.time ?? 0))
}
const loadHistory = attempt(async direction => {
  if (view.historyLoading || view.conversationLoading || !view.current || view.page) return
  const cursor = direction === 'older' ? view.more : view.newer
  if (cursor == null) return
  const conversation = view.current, ticket = navigationTicket, box = $('#messages'), height = box.scrollHeight, top = box.scrollTop
  view.historyLoading = true
  try {
    const page = await call('chat.page', { conversation, [direction === 'older' ? 'before' : 'after']: cursor })
    if (conversation !== view.current || ticket !== navigationTicket) return
    if (direction === 'older') { view.items = mergeMessages(page.items, view.items); view.more = page.more }
    else { view.items = mergeMessages(view.items, page.items); view.newer = page.newer }
    view.restoring = true; renderMessages({ keepScroll: true }); view.restoring = false
    box.scrollTop = direction === 'older' ? top + box.scrollHeight - height : top
  } finally { if (conversation === view.current && ticket === navigationTicket) { view.historyLoading = false; scheduleReading() } }
})
const loadLatest = attempt(async () => {
  if (!view.current || view.page) return
  const conversation = view.current, ticket = navigationTicket
  const page = await call('chat.page', { conversation })
  if (conversation !== view.current || ticket !== navigationTicket) return
  const contiguous = view.newer == null && view.items.length > 0
  view.items = mergeMessages(contiguous ? view.items : [], page.items); view.more = contiguous ? view.more : page.more; view.newer = null
  renderMessages(); $('#messages').scrollTop = $('#messages').scrollHeight; scheduleReading()
})

let readingTimer, readingSignature = ''
function scheduleReading() { clearTimeout(readingTimer); readingTimer = setTimeout(() => { void saveReading() }, 350) }
async function saveReading() {
  if (!view.current || view.page || document.hidden || view.restoring || view.historyLoading || view.conversationLoading) return
  const box = $('#messages'), bounds = box.getBoundingClientRect()
  const visible = [...box.querySelectorAll('.msg[data-id], .notice[data-id], .handoff[data-id]')].filter(el => { const r = el.getBoundingClientRect(); return r.bottom > bounds.top && r.top < bounds.bottom })
  if (!visible.length) return
  const conversation = view.current, through = visible.at(-1).dataset.id, anchor = visible[0].dataset.id, offset = Math.round(visible[0].getBoundingClientRect().top - bounds.top)
  const signature = JSON.stringify([conversation, through, anchor, offset])
  if (signature === readingSignature) return
  readingSignature = signature
  try { const meta = await call('chat.read', { conversation, through, anchor, offset }); if (view.team) { view.team.history ??= {}; view.team.history[conversation] = newerMeta(view.team.history[conversation], meta); renderSidebar() } }
  catch { if (signature === readingSignature) readingSignature = '' }
}

function setSidebar(open) {
  $('#sidebar').classList.toggle('open', open)
  $('#sidebar-shade').hidden = !open
  $('#toggle-sidebar').setAttribute('aria-expanded', open)
}

function rememberPreview(conversation, item) {
  if (!['bot', 'user', 'file'].includes(item?.kind)) return
  view.previews[conversation] = { text: short(item.kind === 'file' ? item.name : item.text, 90), at: item.time ?? item.at }
}

// Everything that waits on the user (a question, an approval, a key) is one kind of card,
// like the welcome prompt: a bold question, a muted line, lettered choices and a free answer.
const ASK_SEND = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M12 19V5"/><path d="M5 12l7-7 7 7"/></svg>'
function askCard({ id, bot, title, sub, body = '', options = [], input = '', dismiss = '' }) {
  const who = botById(bot)
  const head = `<div class="ask-head">${who ? avatar(who, 'tiny') : ''}<div class="ask-text"><div class="ask-title">${title}</div>${sub ? `<p class="ask-sub">${sub}</p>` : ''}</div>${dismiss}</div>`
  const choices = options.length ? `<div class="ask-options">${options.map(([attrs, label], i) => `<button ${attrs}><span>${String.fromCharCode(65 + i)}</span>${escape(label)}</button>`).join('')}</div>` : ''
  return `<div class="ask-card" data-ask="${escape(id)}">${head}${body}${choices}${input}</div>`
}
const askDismiss = (attr, label) => `<button class="icon ask-close" ${attr} aria-label="${escape(label)}" title="${escape(label)}">✕</button>`
const approvalHtml = a => {
  const info = toolInfo({ name: a.tool, input: a.input })
  return askCard({
    id: a.id, bot: a.bot,
    title: t('{who} wants to use {tool}', { who: escape(botName(botById(a.bot)) ?? t('A Bot')), tool: escape(info.label) }),
    sub: info.arg ? escape(info.arg) : escape(t('Allow this step?')),
    body: `<details class="ask-details"><summary>${t('Details')}</summary><pre>${escape(JSON.stringify(a.input, null, 2))}</pre></details>`,
    options: [[`data-allow="${escape(a.id)}"`, t('Allow')], [`data-deny="${escape(a.id)}"`, t('Deny')]],
  })
}
function cardHtml(c) {
  const name = escape(botName(botById(c.bot)) ?? t('A Bot'))
  if (c.kind === 'secret') {
    return askCard({
      id: c.id, bot: c.bot,
      title: t('{who} needs the key {name}', { who: name, name: `<code>${escape(c.name)}</code>` }),
      sub: c.purpose ? escape(c.purpose) : escape(t('The value is stored securely; the Bot never sees it.')),
      options: [[`data-card-dismiss="${escape(c.id)}"`, t('Not now')]],
      input: `<div class="ask-input"><input type="password" autocomplete="off" placeholder="${t('Paste the key')}" data-card-input="${escape(c.id)}"><button class="primary ask-send" data-card="${escape(c.id)}" aria-label="${t('Save')}" title="${t('Save')}">${ASK_SEND}</button></div>`,
    })
  }
  return askCard({
    id: c.id, bot: c.bot,
    title: markdown(c.question),
    sub: escape(t(c.options?.length ? '{who} is waiting for you · pick one, or type your own' : '{who} is waiting for your answer', { who: name })),
    options: (c.options ?? []).map(o => [`data-card="${escape(c.id)}" data-answer="${escape(o)}"`, o]),
    input: `<div class="ask-input"><input placeholder="${t('Type your own answer')}" data-card-input="${escape(c.id)}"><button class="primary ask-send" data-card="${escape(c.id)}" aria-label="${t('Send')}" title="${t('Send')}">${ASK_SEND}</button></div>`,
    dismiss: askDismiss(`data-card-dismiss="${escape(c.id)}"`, t('Dismiss')),
  })
}

function renderApprovals() {
  if (view.page) { stableHtml($('#approvals'), ''); return }
  const room = currentRoom()
  const here = key => key === view.current || (room && key.startsWith(`room:${room.id}:`))
  const mine = (view.work.approvals ?? []).filter(a => here(a.conversation))
  const cards = (view.work.cards ?? []).filter(c => here(c.conversation))
  const others = (view.work.approvals ?? []).length + (view.work.cards ?? []).length - mine.length - cards.length
  const typing = document.activeElement?.dataset?.cardInput
  const kept = typing ? document.activeElement.value : null
  stableHtml($('#approvals'), mine.map(approvalHtml).join('') + cards.map(cardHtml).join('')
    + (others > 0 ? `<button class="link waiting-elsewhere" data-page="dashboard">${t('{n} more waiting for you elsewhere →', { n: others })}</button>` : ''))
  if (typing) { const el = document.querySelector(`[data-card-input="${CSS.escape(typing)}"]`); if (el) { el.value = kept; el.focus() } }
}

let navigationTicket = 0, conversationRequest
const conversationCache = new Map()
function conversationPosition() {
  const box = $('#messages'), bounds = box.getBoundingClientRect()
  const first = [...box.querySelectorAll('[data-id]')].find(el => el.getBoundingClientRect().bottom > bounds.top)
  const anchor = first ? { id: first.dataset.id, offset: first.getBoundingClientRect().top - bounds.top } : null
  return { anchor, top: box.scrollTop }
}
function cacheConversation() {
  if (!view.current || view.page || view.conversationLoading || view.conversationError) return
  conversationCache.delete(view.current)
  conversationCache.set(view.current, { items: [...view.items], more: view.more, newer: view.newer, ...conversationPosition() })
  while (conversationCache.size > 8 || [...conversationCache.values()].reduce((sum, c) => sum + c.items.length, 0) > 2000) conversationCache.delete(conversationCache.keys().next().value)
}
function restoreConversationScroll(anchor, top = 0) {
  const box = $('#messages'), target = anchor && box.querySelector(`[data-id="${CSS.escape(anchor.id)}"]`)
  box.scrollTop = target ? box.scrollTop + target.getBoundingClientRect().top - box.getBoundingClientRect().top - anchor.offset : top
}
const open = async conversation => {
  const ticket = ++navigationTicket
  conversationRequest?.abort()
  void saveReading()
  cacheConversation()
  const same = view.current === conversation
  const cached = conversationCache.get(conversation)
  const retained = same && (view.items.length || cached) ? { items: view.items, more: view.more, newer: view.newer, anchor: cached?.anchor, top: cached?.top } : cached
  const hasWindow = Boolean(cached || (same && !view.conversationLoading && !view.conversationError && view.items.length))
  stashComposer()
  $('#settings').hidden = true
  $('#app').inert = false
  view.page = null
  view.current = conversation
  view.editing = null
  view.items = retained ? [...retained.items] : []
  view.more = retained?.more ?? null; view.newer = retained?.newer ?? null; view.historyLoading = false
  view.conversationLoading = true; view.conversationError = null
  view.searchTicket++; view.searchTotal = 0; view.searchMatch = null
  restoreComposer(conversation)
  $('#chat-search').hidden = true
  $('#chat-search-input').value = ''
  view.searchIndex = 0
  history.replaceState(null, '', `#${encodeURIComponent(conversation)}`)
  setSidebar(false)
  view.restoring = true
  renderAll()
  if (retained) restoreConversationScroll(retained.anchor, retained.top)
  else $('#messages').scrollTop = 0
  view.restoring = false
  // Mount the correct workspace once, independently of the history request.
  if (!same && view.panel) openPanel(view.panel)
  const reading = retained?.anchor ?? view.team?.history?.[conversation]?.reading
  const controller = new AbortController(); conversationRequest = controller
  const timeout = setTimeout(() => controller.abort(), 15000)
  const initialItems = new Map(view.items.map(item => [item.id, item]))
  try {
    const page = await call('chat.page', { conversation, anchor: reading?.id ?? reading?.anchor }, { signal: controller.signal })
    if (view.current !== conversation || view.page || ticket !== navigationTicket) return
    const position = retained ? conversationPosition() : null
    const liveUpdates = view.items.filter(item => initialItems.get(item.id) !== item)
    view.items = mergeMessages(mergeMessages(view.items, page.items), liveUpdates)
    view.more = hasWindow ? (retained.more == null || page.more == null ? null : Math.min(retained.more, page.more)) : page.more
    view.newer = hasWindow ? (retained.newer == null || page.newer == null ? null : Math.max(retained.newer, page.newer)) : page.newer
    view.conversationLoading = false
    const last = [...page.items].reverse().find(i => ['bot', 'user', 'file'].includes(i.kind))
    if (last) rememberPreview(conversation, last)
    renderSidebar()
    const anchor = position?.anchor ?? (reading && { id: reading.id ?? reading.anchor, offset: reading.offset ?? 0 })
    view.restoring = Boolean(anchor?.id)
    renderMessages({ keepScroll: Boolean(retained) })
    if (view.restoring) restoreConversationScroll(anchor, position?.top ?? retained?.top)
    view.restoring = false; cacheConversation(); scheduleReading()
  } catch (error) {
    if (ticket !== navigationTicket || view.current !== conversation || view.page) return
    view.conversationLoading = false; view.conversationError = error
    renderMessages({ keepScroll: true })
    if (view.items.length) toast(t('Refresh failed. Showing saved content; try again.'))
  } finally { clearTimeout(timeout); if (conversationRequest === controller) conversationRequest = null }
}

function openPage(page) {
  if (!PAGES[page]) return
  if (!view.page) { view.settingsPanel = view.panel; void saveReading(); cacheConversation() }
  else rememberSettingsPage()
  if (view.page !== page) $('#settings-content').dataset.renderedPage = ''
  ++navigationTicket; conversationRequest?.abort()
  view.conversationLoading = false
  view.page = page
  $('#settings').hidden = false
  $('#app').inert = true
  history.replaceState(null, '', `#page:${page}`)
  setSidebar(false)
  closePanel(false)
  renderAll()
  $('#settings-title').focus()
  void loadPage(page, true, false)
}

function closeSettings() {
  rememberSettingsPage()
  const panel = view.settingsPanel
  view.panel = panel
  const conversation = view.current || bots()[0]?.conversation
  if (conversation) { void open(conversation); if (panel) openPanel(panel) }
  else { view.page = null; $('#settings').hidden = true; $('#app').inert = false; history.replaceState(null, '', location.pathname); renderAll() }
  $('#open-settings').focus()
}

function renderAll() {
  renderSidebar()
  renderHead()
  if (view.page === 'channels' && $('#delivery-list')) refreshChannelCards()
  else renderMessages({ keepScroll: true })
  renderApprovals()
}

// ---------------------------------------------------------------------------
// Team pages

let pageData = {}
const pageLoads = new Map(), pageVersions = new Map(), pageStatus = new Map(), settingsSnapshots = new Map()
let pageLoadSerial = 0
function settingsReady(page) {
  if (page === 'routines') return Boolean(pageData.routines && pageData.routineHistory)
  if (page === 'providers') return Boolean(pageData.providers)
  return true
}
function rememberSettingsPage() {
  const box = $('#settings-content')
  if (!view.page || box.dataset.renderedPage !== view.page || box.querySelector('.loading-skeleton, .load-error')) return
  settingsSnapshots.set(view.page, { draft: settingsDraft().map(field => ({ ...field, focus: false })), scroll: box.scrollTop })
}
function renderPageLoading() {
  const status = pageStatus.get(view.page), loading = Boolean(status?.loading || (!settingsReady(view.page) && !status?.error))
  $('#settings-content').setAttribute('aria-busy', String(loading))
  $('#settings-loading').hidden = !loading
  $('#settings-retry').hidden = !status?.error
}
// A silent load is a background poll: it neither shows the header indicator nor toasts on failure.
const loadPage = async (page, preserveDraft = true, force = true, { silent = false } = {}) => {
  if (!['routines', 'providers', 'connections'].includes(page)) return
  const key = page
  const pending = pageLoads.get(key)
  if (pending && silent) return pending.request
  if (pending) {
    pageVersions.set(page, pending.version)
    pageStatus.set(page, { ...pageStatus.get(page), key, loading: true, error: null })
    if (view.page === page) renderPageLoading()
    return pending.request
  }
  const previous = pageStatus.get(page)
  if (!force && settingsReady(page) && previous?.key === key && !previous.error && Date.now() - previous.loadedAt < 15000) return
  const version = ++pageLoadSerial
  pageVersions.set(page, version)
  pageStatus.set(page, { ...previous, key, loading: !silent, error: null })
  if (view.page === page && !silent) renderPageLoading()
  const request = (async () => {
    try {
      let data
      const options = { signal: AbortSignal.timeout(15000) }
      if (page === 'routines') { const [routines, routineHistory] = await Promise.all([call('routine.list', {}, options), call('routine.history', {}, options)]); data = { routines, routineHistory } }
      if (page === 'providers') data = { providers: await call('provider.list', {}, options) }
      if (version !== pageVersions.get(page)) return
      Object.assign(pageData, data)
      pageStatus.set(page, { key, loading: false, error: null, loadedAt: Date.now() })
      if (view.page === page) renderPage({ preserveDraft })
    } catch (error) {
      if (version !== pageVersions.get(page)) return
      pageStatus.set(page, { ...previous, key, loading: false, error })
      if (view.page === page) { renderPage({ preserveDraft: true }); if (settingsReady(page) && !silent) toast(t('Refresh failed. Showing saved content; try again.')) }
    } finally { pageLoads.delete(key) }
  })()
  pageLoads.set(key, { request, version })
  return request
}

function settingsDraft() {
  return [...$('#settings-content').querySelectorAll('input[id]:not([type=file]):not([data-auth-secret]), textarea[id], select[id]')].map(el => ({ id: el.id, value: el.value, checked: el.checked, focus: el === document.activeElement, start: el.selectionStart, end: el.selectionEnd }))
}
function restoreSettingsDraft(draft) {
  const kind = draft.find(field => field.id === 'rt-kind')
  if (kind && $('#rt-kind')) { $('#rt-kind').value = kind.value; $('#rt-kind').onchange?.() }
  queueMicrotask(() => $('#rt-kind')?.onchange?.())
  for (const field of draft) {
    const el = document.getElementById(field.id)
    if (!el) continue
    el.value = field.value; if (el.type === 'checkbox') el.checked = field.checked
    if (field.focus) { el.focus({ preventScroll: true }); if (field.start !== null && ['text', 'search', 'password', 'url', 'tel', 'textarea'].includes(el.type)) el.setSelectionRange(field.start, field.end) }
  }
}

function renderPage({ preserveDraft = false } = {}) {
  if (!view.page) return
  const box = $('#settings-content'), changedPage = box.dataset.renderedPage !== view.page
  const snapshot = settingsSnapshots.get(view.page)
  const draft = changedPage ? snapshot?.draft ?? [] : preserveDraft ? settingsDraft() : []
  const scroll = changedPage ? snapshot?.scroll ?? 0 : box.scrollTop
  if (changedPage || !preserveDraft) renderedMarkup.delete(box)
  box.dataset.renderedPage = view.page
  $('#settings-title').textContent = t(PAGES[view.page])
  renderPageLoading()
  if (!settingsReady(view.page)) {
    stableHtml(box, pageStatus.get(view.page)?.error ? loadErrorHtml('data-page-retry') : skeletonHtml('settings'))
    box.scrollTop = 0
    return
  }
  if (view.page === 'general') {
    if (changedPage || !$('#p-name')) { renderGeneralSettings(); restoreSettingsDraft(draft); box.scrollTop = scroll }
    return
  }
  const html = { dashboard: dashboardHtml, routines: routinesHtml, skills: skillsHtml, connections: connectionsHtml, channels: channelsHtml, providers: providersHtml }[view.page]?.()
  if (html === undefined) return
  if (stableHtml(box, html)) {
    if (view.page === 'providers') bindProviders(box)
    if (draft.length) restoreSettingsDraft(draft)
    box.scrollTop = changedPage || preserveDraft ? scroll : 0
  }
  refreshDetail()
}

// Providers, as cc-switch has them: cards for where the native engine sends its requests, one in
// use for the whole team. Each keeps its models, a default and a model per task.
const modelList = () => `<datalist id="provider-models">${(view.team.provider?.models ?? []).map(m => `<option value="${escape(m)}">`).join('')}</datalist>`
const PRESETS = {
  text: [
    { preset: 'anthropic', api: 'anthropic', auth: 'key', name: 'Anthropic', baseUrl: 'https://api.anthropic.com', models: [] },
    { preset: 'deepseek', api: 'anthropic', name: 'DeepSeek', baseUrl: 'https://api.deepseek.com/anthropic', models: ['deepseek-chat', 'deepseek-reasoner'] },
    { preset: 'kimi', api: 'anthropic', name: 'Kimi', baseUrl: 'https://api.moonshot.cn/anthropic', models: [] },
    { preset: 'glm', api: 'anthropic', name: '智谱 GLM', baseUrl: 'https://open.bigmodel.cn/api/anthropic', models: [] },
    { preset: 'minimax', api: 'anthropic', name: 'MiniMax', baseUrl: 'https://api.minimaxi.com/anthropic', models: [] },
    { preset: 'openrouter', api: 'anthropic', name: 'OpenRouter', baseUrl: 'https://openrouter.ai/api', models: [] },
    { preset: 'openai', api: 'openai', name: 'OpenAI', baseUrl: 'https://api.openai.com/v1', models: [] },
    { preset: 'qwen', api: 'openai', name: '通义千问', baseUrl: 'https://dashscope.aliyuncs.com/compatible-mode/v1', models: [] },
    { preset: 'siliconflow', api: 'openai', name: '硅基流动', baseUrl: 'https://api.siliconflow.cn/v1', models: [] },
    { preset: 'ollama', api: 'openai', name: 'Ollama', baseUrl: 'http://127.0.0.1:11434/v1', models: [] },
    { preset: 'custom', api: 'anthropic', name: '', baseUrl: '', models: [], label: 'Custom (Anthropic)' },
    { preset: 'custom', api: 'openai', name: '', baseUrl: '', models: [], label: 'Custom (OpenAI)' },
  ],
  image: [
    { preset: 'openai', name: 'OpenAI', baseUrl: 'https://api.openai.com/v1', models: ['gpt-image-1'] },
    { preset: 'zhipu', name: '智谱 CogView', baseUrl: 'https://open.bigmodel.cn/api/paas/v4', models: ['cogview-4'] },
    { preset: 'ark', name: '豆包 Seedream', baseUrl: 'https://ark.cn-beijing.volces.com/api/v3', models: [] },
    { preset: 'siliconflow', name: '硅基流动', baseUrl: 'https://api.siliconflow.cn/v1', models: [] },
    { preset: 'custom', name: '', baseUrl: '', models: [] },
  ],
  video: [
    { preset: 'zhipu', format: 'zhipu', name: '智谱 CogVideoX', baseUrl: 'https://open.bigmodel.cn/api/paas/v4', models: [] },
    { preset: 'ark', format: 'ark', name: '豆包 Seedance', baseUrl: 'https://ark.cn-beijing.volces.com/api/v3', models: [] },
    { preset: 'siliconflow', format: 'siliconflow', name: '硅基流动', baseUrl: 'https://api.siliconflow.cn/v1', models: [] },
    { preset: 'custom', format: 'zhipu', name: '', baseUrl: '', models: [] },
  ],
}
const KIND_NAMES = { text: 'Text', image: 'Image generation', video: 'Video generation' }
const JOB_NAMES = { approve: 'Auto-approval', review: 'Memory review', memory: 'Memory panel', planning: 'Pre-reply planning' }
const API_NAMES = { anthropic: 'Anthropic Messages', openai: 'OpenAI Chat Completions' }
const FORMAT_NAMES = { zhipu: '智谱 (/videos/generations)', ark: '火山方舟 (/contents/generations/tasks)', siliconflow: '硅基流动 (/video/submit)' }
const providerView = { kind: 'text' }
const providerTests = {}
const providerName = p => p.name
function providerCard(p) {
  const media = p.kind !== 'text'
  const test = providerTests[p.id]
  const model = p.defaultModel || p.models?.[0] || '—'
  const detail = !media ? (p.contextWindow ? `${t('Context window')} ${p.contextWindow.toLocaleString()}` : '') : [p.kind === 'video' ? FORMAT_NAMES[p.format] ?? p.format : '', p.size ? `${t('Size')} ${p.size}` : ''].filter(Boolean).join(' · ')
  const testText = !test ? '' : test.pending ? t('Testing…') : !test.ok ? escape(t(test.error || 'Failed'))
    : media ? (test.note === 'unchecked' ? t('Reached · {ms} ms · the key is checked on first use', { ms: test.ms }) : t('Key accepted · {ms} ms', { ms: test.ms }))
    : t('Works · {ms} ms · {model}', { ms: test.ms, model: escape(test.model || t('default')) })
  const buttons = [
    p.active ? (media ? `<button class="ghost" data-provider-stop="${escape(p.kind)}">${t('Stop using')}</button>` : '') : `<button class="primary" data-provider-use="${escape(p.id)}">${t('Use')}</button>`,
    `<button class="ghost" data-provider-test="${escape(p.id)}">${t('Test')}</button>`,
    `<button class="ghost" data-provider-edit="${escape(p.id)}">${t('Edit')}</button>`,
    p.active ? '' : `<button class="danger" data-provider-delete="${escape(p.id)}">${t('Remove')}</button>`,
  ].join('')
  return `<div class="provider ${p.active ? 'active' : ''}">
    <div class="provider-head"><strong>${escape(providerName(p))}</strong>${p.active ? `<span class="badge on">${t('In use')}</span>` : ''}</div>
    <div class="muted provider-url">${media ? '' : `${escape(API_NAMES[p.api] ?? API_NAMES.anthropic)} · `}${escape(p.baseUrl)}${p.keyHint ? ` · ${t('key')} ${escape(p.keyHint)}` : p.api === 'openai' ? '' : ` · <span class="warn-text">${t('no key')}</span>`}</div>
    <div class="provider-model">${t(media ? 'Model' : 'Default model')}: <code>${escape(model)}</code></div>
    ${media ? '' : `<label class="provider-effort">${t('Default reasoning effort')}<select data-provider-effort="${escape(p.id)}" aria-label="${t('Default reasoning effort')}">${effortOptions(p.defaultEffort ?? '', t('Not set (the model decides)'))}</select></label>`}
    ${detail ? `<div class="muted provider-tasks">${escape(detail)}</div>` : ''}
    ${test ? `<div class="provider-test ${test.ok ? 'ok' : test.pending ? '' : 'err'}">${testText}</div>` : ''}
    <div class="row-actions">${buttons}</div></div>`
}
function providersHtml() {
  const data = pageData.providers
  if (!data) return `<div class="page muted">${t('Loading…')}</div>`
  const kind = providerView.kind
  const list = data.providers.filter(p => (p.kind || 'text') === kind)
  const intro = {
    text: t('Where the built-in pi-agent engine sends the team\'s requests, over the Anthropic Messages or OpenAI Chat Completions protocol, with your API key (local Ollama needs none). One is in use for every Bot; switching takes effect from each conversation\'s next turn, which starts fresh on the new provider while memory carries over.'),
    image: t('The service behind the Bots\' generate_image tool (OpenAI-style images API). With one in use, a Bot can make pictures; they show in the chat and are saved in its working folder.'),
    video: t('The service behind the Bots\' generate_video tool. Video services differ, so pick the API format that matches yours. Videos take minutes; they show in the chat when done.'),
  }[kind]
  // Image or video off: say what that means and offer the next step, using a saved one or adding one.
  const tool = kind === 'image' ? 'generate_image' : 'generate_video'
  const ready = list.filter(p => p.hasKey)
  const noText = kind === 'text' && !list.some(p => p.active) ? `<div class="media-off">
    <strong>${t('No text provider is in use: Bots cannot answer yet')}</strong>
    <div class="row-actions">${list.filter(p => p.hasKey || p.api === 'openai').map(p => `<button class="primary" data-provider-use="${escape(p.id)}">${t('Use {name}', { name: escape(p.name) })}</button>`).join('')}<button class="${list.length ? 'ghost' : 'primary'}" data-provider-add>${t('Add a provider')}</button></div></div>` : ''
  const off = kind === 'text' || list.some(p => p.active) ? noText : `<div class="media-off">
    <strong>${t(kind === 'image' ? 'Image generation is off' : 'Video generation is off')}</strong>
    <div class="row-actions">${ready.map(p => `<button class="primary" data-provider-use="${escape(p.id)}">${t('Use {name}', { name: escape(p.name) })}</button>`).join('')}<button class="${ready.length ? 'ghost' : 'primary'}" data-provider-add>${t('Add a provider')}</button></div></div>`
  // Background jobs name a model of the provider in use; empty runs them on its default model.
  const active = list.find(p => p.active)
  const fallback = active ? active.defaultModel || active.models?.[0] || '' : ''
  const jobs = kind !== 'text' || !active ? '' : `<h3>${t('Models for background jobs')}</h3>
    <p class="muted">${t('Each job runs on a model of {name}. Empty uses its default model; a fast model suits pre-reply planning and auto-approval.', { name: escape(active.name) })}</p>
    <datalist id="job-models">${(active.models ?? []).map(m => `<option value="${escape(m)}">`).join('')}</datalist>
    <div class="form-grid">${Object.keys(JOB_NAMES).map(j => `<label>${t(JOB_NAMES[j])}<input data-job="${j}" list="job-models" value="${escape(data.jobs[j] ?? '')}" placeholder="${escape(fallback ? t('Default ({model})', { model: fallback }) : t('Default'))}"></label>`).join('')}</div>`
  return `<div class="page">
    <div class="seg kind-tabs" role="tablist">${Object.entries(KIND_NAMES).map(([k, n]) => `<button data-provider-kind="${k}" class="${k === kind ? 'on' : ''}" aria-pressed="${k === kind}">${t(n)}</button>`).join('')}</div>
    <p class="muted">${intro}</p>${off}
    <div class="provider-grid">${list.map(providerCard).join('') || `<div class="muted">${t('None yet. Add one below.')}</div>`}</div>
    ${jobs}
    <h3>${t('Add a provider')}</h3><div class="preset-row">${PRESETS[kind].map((p, i) => `<button class="ghost" data-provider-preset="${i}">${escape(p.preset === 'custom' ? t(p.label ?? 'Custom') : p.name)}</button>`).join('')}</div>
    </div>`
}
function providerModal(p) {
  const kind = p.kind || 'text'
  const media = kind !== 'text'
  const list = () => $('#pv-models').value.split(/[\n,]/).map(s => s.trim()).filter(Boolean)
  const api = () => $('#pv-api')?.value ?? p.api ?? 'anthropic'
  const title = p.id ? t('Edit {name}', { name: escape(providerName(p)) }) : t('Add a provider · {kind}', { kind: t(KIND_NAMES[kind]) })
  const urlLabel = () => media ? t('Address (the API root, such as https://api.openai.com/v1)') : api() === 'openai' ? t('Address (OpenAI-compatible, usually ending in /v1)') : t('Address (Anthropic-compatible)')
  modal(`<h3>${title}</h3>
    <div class="form-grid pv-form">
    <label class="${media && kind !== 'video' ? 'full' : ''}">${t('Name')}<input id="pv-name" value="${escape(p.name ?? '')}" placeholder="${t('My provider')}"></label>
    ${media ? '' : `<label>${t('Protocol')}<select id="pv-api">${Object.entries(API_NAMES).map(([v, n]) => `<option value="${v}" ${(p.api || 'anthropic') === v ? 'selected' : ''}>${escape(n)}</option>`).join('')}</select></label>`}
    ${kind === 'video' ? `<label>${t('API format')}<select id="pv-format">${Object.entries(FORMAT_NAMES).map(([v, n]) => `<option value="${v}" ${(p.format || 'zhipu') === v ? 'selected' : ''}>${escape(n)}</option>`).join('')}</select></label>` : ''}
    <label class="full"><span id="pv-url-label">${urlLabel()}</span><input id="pv-url" value="${escape(p.baseUrl ?? '')}" placeholder="${media || p.api === 'openai' ? 'https://api.example.com/v1' : 'https://api.example.com/anthropic'}"></label>
    <label class="${media ? 'full' : ''}">${t('API key')}<input id="pv-key" type="password" autocomplete="off" placeholder="${p.hasKey ? t('Saved ({hint}); leave empty to keep it', { hint: p.keyHint }) : 'sk-…'}"></label>
    ${media ? '' : `<label id="pv-auth-row" ${p.api === 'openai' ? 'hidden' : ''}>${t('Send the key as')}<select id="pv-auth"><option value="token" ${p.auth !== 'key' ? 'selected' : ''}>Authorization: Bearer</option><option value="key" ${p.auth === 'key' ? 'selected' : ''}>x-api-key</option></select></label>`}
    <div class="full pv-models-block"><label for="pv-models">${t('Models, one per line')}</label><div class="pv-models-row"><textarea id="pv-models" rows="4">${escape((p.models ?? []).join('\n'))}</textarea><button class="ghost" id="pv-fetch" type="button">${t('Fetch models')}</button></div></div>
    <datalist id="pv-list"></datalist>
    <label class="${media ? '' : ''}">${t(media ? 'Model' : 'Default model')}<input id="pv-default" list="pv-list" value="${escape(p.defaultModel ?? '')}" placeholder="${t('The first model')}"></label>
    ${media ? `<label>${t('Default size (optional)')}<input id="pv-size" value="${escape(p.size ?? '')}" placeholder="${kind === 'image' ? '1024x1024' : '1280x720'}"></label>`
      : `<label>${t('Context window (tokens)')}<input id="pv-context" type="number" min="8000" step="1000" value="${p.contextWindow || ''}" placeholder="128000"></label>
      <label class="full">${t('Default reasoning effort')}<select id="pv-effort">${effortOptions(p.defaultEffort ?? '', t('Not set (the model decides)'))}</select></label>
      <p class="full muted field-hint">${t('Used by every Bot without a reasoning effort of its own; a Bot\'s own setting wins. Needs a model that supports reasoning: others may refuse the request. Background jobs keep their own defaults.')}</p>
      <p class="full muted field-hint">${t('Bots and background jobs name one of these models; the default model runs whenever none is named.')} ${t('Before a conversation would not fit, the engine summarizes its older part. Use the smallest window of the models you use here.')}</p>`}
    </div>
    <div class="form-actions"><button class="ghost" data-close>${t('Cancel')}</button><button class="primary" id="pv-save">${t('Save')}</button></div>`, true)
  const fill = () => { $('#pv-list').innerHTML = list().map(m => `<option value="${escape(m)}">`).join('') }
  fill()
  $('#pv-models').oninput = fill
  if ($('#pv-api')) $('#pv-api').onchange = () => { $('#pv-url-label').textContent = urlLabel(); $('#pv-auth-row').hidden = api() === 'openai' }
  $('#pv-fetch').onclick = attempt(async () => {
    $('#pv-fetch').disabled = true
    try {
      const models = await call('provider.models', { id: p.id ?? '', kind, api: api(), baseUrl: $('#pv-url')?.value ?? '', apiKey: $('#pv-key')?.value ?? '' })
      $('#pv-models').value = [...new Set([...list(), ...models])].join('\n'); fill()
      toast(t('{n} models', { n: models.length }))
    } finally { $('#pv-fetch').disabled = false }
  })
  $('#pv-save').onclick = attempt(async () => {
    await call('provider.save', {
      id: p.id ?? '', kind, preset: p.preset ?? 'custom', name: $('#pv-name')?.value ?? p.name, baseUrl: $('#pv-url')?.value ?? '', apiKey: $('#pv-key')?.value ?? '',
      api: media ? 'anthropic' : api(), auth: api() === 'openai' ? 'token' : ($('#pv-auth')?.value ?? 'token'), format: $('#pv-format')?.value ?? '', size: $('#pv-size')?.value ?? '', models: list(), defaultModel: $('#pv-default').value, contextWindow: Number($('#pv-context')?.value || 0), defaultEffort: $('#pv-effort')?.value ?? '',
    })
    closeModal(); toast(t('Saved')); loadPage('providers')
  })
}
function bindProviders(box) {
  const find = id => pageData.providers.providers.find(p => p.id === id)
  for (const b of box.querySelectorAll('[data-provider-kind]')) b.onclick = () => { providerView.kind = b.dataset.providerKind; renderPage() }
  for (const b of box.querySelectorAll('[data-provider-add]')) b.onclick = () => box.querySelector('.preset-row')?.scrollIntoView({ behavior: 'smooth', block: 'center' })
  for (const b of box.querySelectorAll('[data-provider-preset]')) b.onclick = () => providerModal({ auth: 'token', ...PRESETS[providerView.kind][Number(b.dataset.providerPreset)], kind: providerView.kind })
  for (const b of box.querySelectorAll('[data-provider-edit]')) b.onclick = () => providerModal(find(b.dataset.providerEdit))
  for (const select of box.querySelectorAll('[data-provider-effort]')) select.onchange = attempt(async () => {
    await call('provider.effort', { id: select.dataset.providerEffort, effort: select.value })
    toast(t(select.value ? 'Default reasoning effort saved: {level}' : 'Default reasoning effort cleared', { level: effortLabel(select.value) }))
    loadPage('providers')
  })
  for (const b of box.querySelectorAll('[data-provider-use]')) b.onclick = attempt(async () => {
    const p = find(b.dataset.providerUse)
    await call('provider.use', { id: p.id })
    toast(p.kind === 'text' ? t('Now on {name}: every Bot uses it from its next turn', { name: providerName(p) }) : t('{name} in use: Bots get the tool from their next turn', { name: p.name }))
    loadPage('providers')
  })
  for (const b of box.querySelectorAll('[data-provider-stop]')) b.onclick = attempt(async () => { await call('provider.stop', { kind: b.dataset.providerStop }); toast(t('Stopped: Bots lose the tool from their next turn')); loadPage('providers') })
  for (const b of box.querySelectorAll('[data-provider-delete]')) b.onclick = attempt(async () => { const p = find(b.dataset.providerDelete); if (confirm(t('Remove {name}?', { name: p.name }))) { await call('provider.delete', { id: p.id }); loadPage('providers') } })
  for (const s of box.querySelectorAll('[data-job]')) s.onchange = attempt(async () => { const model = s.value.trim(); await call('provider.jobs', { jobs: { [s.dataset.job]: model } }); pageData.providers.jobs[s.dataset.job] = model; toast(t('Saved')) })
  for (const b of box.querySelectorAll('[data-provider-test]')) b.onclick = async () => {
    const id = b.dataset.providerTest
    providerTests[id] = { pending: true }; renderPage()
    try { providerTests[id] = await call('provider.test', { id }) } catch (error) { providerTests[id] = { ok: false, error: error.message } }
    if (view.page === 'providers') renderPage()
  }
}

function dashboardHtml() {
  const waiting = [...(view.work.approvals ?? []).map(approvalHtml), ...(view.work.cards ?? []).map(cardHtml)]
  const cards = bots().map((bot) => {
    const doing = botActivity(bot)
    const run = botRuns(bot)[0]
    const shot = view.work.shots?.[bot.id]
    return `<button class="bot-card" data-open="${escape(bot.conversation)}">
      <div class="bot-card-head">${avatar(bot, doing === 'waiting' ? 'waiting' : doing ? 'busy' : '')}<div><div class="name">${escape(bot.name)}${bot.main ? `<span class="badge">${t('Main')}</span>` : ''}</div><div class="muted">${escape(bot.role || t('Bot'))}</div></div></div>
      <div class="bot-card-status ${doing ? 'on' : ''}">${doing ? `${escape(doingText(doing, botDetail(bot)))}${run ? ` · ${t('since {time}', { time: new Date(run[1].since).toLocaleTimeString(locale(), { hour: '2-digit', minute: '2-digit' }) })}` : ''}` : t('Idle')}</div>
      ${bot.description ? `<div class="muted clamp">${escape(bot.description)}</div>` : ''}
      ${shot ? `<img class="bot-card-shot" src="/${escape(shot)}" alt="">` : ''}</button>`
  }).join('')
  const tasks = (view.work.tasks ?? []).slice(0, 30).map(task => {
    const source = task.source.split(':').slice(0, 2).join(':')
    const from = bots().find(b => b.id === task.source.split(':').at(-1))?.name ?? t('Bot')
    const to = bots().find(b => b.id === task.target)?.name ?? t('Deleted Bot')
    const active = ['queued', 'running'].includes(task.status)
    return `<details class="row-card task-card"><summary><strong>${escape(from)} → ${escape(to)}</strong> <span class="badge">${t(task.status)}</span><span class="muted">${when(task.created)}</span><div class="muted clamp">${escape(short(taskObjective(task), 120))}</div></summary>
      <p class="muted">${escape(task.id)} · ${escape(task.path.map(id => botById(id)?.name ?? t('Deleted Bot')).join(' → '))}</p>${taskRequirements(task)}${task.result ? `<div class="bubble-plain">${markdown(task.result)}</div>` : ''}
      <div class="form-actions"><button class="ghost" data-open="${escape(source)}">${t('View conversation')}</button>${active ? `<button class="danger" data-task-cancel="${escape(task.id)}">${t('Stop task')}</button>` : ''}</div></details>`
  }).join('')
  return `<div class="page">${waiting.length ? `<h3>${t('Waiting for you')}</h3><div class="stack">${waiting.join('')}</div>` : ''}<h3>${t('Bots')}</h3><div class="grid">${cards}</div>${tasks ? `<h3>${t('Delegated tasks')}</h3><div class="stack">${tasks}</div>` : ''}</div>`
}

// ---------------------------------------------------------------------------
// Compact card lists with search and pages, and a detail window per card. Pages
// register what a list shows; typing or paging only redraws the list body.
const LISTS = {}
const listStates = {}
const listState = key => (listStates[key] ??= { q: '', page: 0 })
function listBodyHtml(key) {
  const def = LISTS[key], state = listState(key), query = state.q.trim().toLowerCase()
  const all = def.items()
  const found = query ? all.filter(item => def.text(item).toLowerCase().includes(query)) : all
  const pages = Math.max(1, Math.ceil(found.length / def.size))
  state.page = Math.min(Math.max(state.page, 0), pages - 1)
  const shown = found.slice(state.page * def.size, (state.page + 1) * def.size)
  const pager = pages > 1
    ? `<div class="pager" role="navigation" aria-label="${t('Pages')}"><button class="ghost" data-list-page="${key}:${state.page - 1}" ${state.page === 0 ? 'disabled' : ''} aria-label="${t('Previous page')}">‹</button><span>${state.page + 1} / ${pages} · ${t('{n} items', { n: found.length })}</span><button class="ghost" data-list-page="${key}:${state.page + 1}" ${state.page >= pages - 1 ? 'disabled' : ''} aria-label="${t('Next page')}">›</button></div>`
    : found.length ? `<p class="pager-count muted">${t('{n} items', { n: found.length })}</p>` : ''
  return `<div class="mini-grid">${shown.map(def.card).join('') || `<p class="muted mini-empty">${def.empty(query)}</p>`}</div>${pager}`
}
function listHtml(key, def, toolbar = '') {
  LISTS[key] = { size: 12, text: item => String(item?.name ?? ''), empty: () => t('Nothing here yet.'), ...def }
  return `<div class="list-box" data-list-box="${key}"><div class="list-bar"><label class="list-search">${icon('search')}<input id="ls-${key}" type="search" data-list-search="${key}" value="${escape(listState(key).q)}" placeholder="${escape(def.placeholder ?? t('Search'))}" autocomplete="off" aria-label="${escape(def.placeholder ?? t('Search'))}"></label>${toolbar}</div><div class="list-body" data-list-body="${key}">${listBodyHtml(key)}</div></div>`
}
function updateList(key) {
  const body = document.querySelector(`[data-list-body="${key}"]`)
  if (body && LISTS[key]) body.innerHTML = listBodyHtml(key)
}
const resetList = key => { listState(key).page = 0 }

// A detail window: DETAILS[kind](id) returns its html, or '' when the item is gone.
const DETAILS = {}
let detailOpen = null, detailMarkup = ''
function openDetail(kind, id) {
  const html = DETAILS[kind]?.(id)
  if (!html) return
  modal(`<button class="icon detail-close" data-close aria-label="${t('Close')}" title="${t('Close')}">✕</button>${html}`, true)
  detailOpen = { kind, id }; detailMarkup = html
}
function refreshDetail() {
  if (!detailOpen || $('#modal').hidden) return
  const html = DETAILS[detailOpen.kind]?.(detailOpen.id)
  if (!html) { closeModal(); return }
  if (html === detailMarkup) return
  const card = $('#modal-card'), opened = new Set([...card.querySelectorAll('details[open]')].map(el => el.dataset.keep))
  card.innerHTML = `<button class="icon detail-close" data-close aria-label="${t('Close')}" title="${t('Close')}">✕</button>${html}`
  for (const el of card.querySelectorAll('details')) if (opened.has(el.dataset.keep)) el.open = true
  detailMarkup = html
}

let routineEditing = null
let routineTarget = null
const routineTargets = () => [...bots().map(b => [b.conversation, b.name]), ...rooms().map(r => [r.conversation, `# ${r.name}`])]
const routineRuns = id => (pageData.routineHistory ?? []).filter(h => h.routine === id)
const routineLive = runs => runs.find(h => ['queued', 'running'].includes(h.status))
const runTone = status => ({ queued: 'live', running: 'live', completed: 'ok', failed: 'err', interrupted: 'warn', cancelled: 'warn' })[status] ?? ''
const runTook = h => {
  if (!h.finished) return ''
  const seconds = Math.max(0, Math.round((h.finished - h.started) / 1000))
  return seconds < 60 ? t('{n}s', { n: seconds }) : t('{n} min', { n: Math.round(seconds / 60) })
}
// Paused stops the schedule only; a run already underway still shows as running.
function routineState(r, live) {
  if (live) return { label: live.status === 'queued' ? 'Queued' : 'Running', tone: 'live' }
  return r.enabled ? { label: 'Active', tone: 'on' } : { label: 'Paused', tone: 'off' }
}
function routineCard(r) {
  const target = routineTargets().find(([k]) => k === r.target)?.[1] ?? r.target
  const event = ['webhook', 'event'].includes(r.when.kind)
  const runs = routineRuns(r.id), live = routineLive(runs), last = runs.find(h => h !== live), state = routineState(r, live)
  const lastText = last ? `<span class="run-mark ${runTone(last.status)}">${t(last.status)}</span>` : ''
  return `<button class="mini-card ${r.enabled ? '' : 'is-off'}" data-detail="routine:${escape(r.id)}"><span class="mini-icon">${icon('routines')}</span><span class="mini-main"><strong>${escape(r.title)}</strong><small>${escape(describeWhen(r.when))} · → ${escape(target)}</small><small>${event ? '' : `${t('Next {next}', { next: r.enabled ? when(r.next) : '—' })}${lastText ? ' · ' : ''}`}${lastText ? `${t('Last run')} ${lastText}` : ''}</small></span><span class="mini-status routine-state ${state.tone}">${state.tone === 'live' ? '<i class="live-dot"></i>' : ''}${t(state.label)}</span></button>`
}
function routineRunHtml(h) {
  const took = runTook(h), text = h.result ?? ''
  const result = !text ? '' : text.length > 240 ? `<details class="run-result"><summary>${escape(short(text, 240))}</summary><div>${escape(text)}</div></details>` : `<div class="run-result">${escape(text)}</div>`
  return `<li class="routine-run ${runTone(h.status)}"><div class="run-line"><span class="run-mark ${runTone(h.status)}">${runTone(h.status) === 'live' ? '<i class="live-dot"></i>' : ''}${t(h.status)}</span><span>${when(h.started)}</span><span class="muted">${t(h.trigger)}</span>${took ? `<span class="muted">${t('took {time}', { time: took })}</span>` : ''}</div>${result}</li>`
}
DETAILS.routine = id => {
  const r = (pageData.routines ?? []).find(x => x.id === id)
  if (!r) return ''
  const runs = routineRuns(r.id), live = routineLive(runs), state = routineState(r, live), event = ['webhook', 'event'].includes(r.when.kind)
  const target = routineTargets().find(([k]) => k === r.target)?.[1] ?? r.target
  const shown = runs.slice(0, 10)
  return `<div class="detail-head"><h3>${escape(r.title)}</h3><span class="badge routine-state ${state.tone}">${state.tone === 'live' ? '<i class="live-dot"></i>' : ''}${t(state.label)}</span></div>
    <p class="muted">→ ${escape(target)} · ${escape(describeWhen(r.when))}${event || r.when.kind === 'every' ? '' : ` · ${escape(r.timezone ?? t('Server local time'))}`}</p>
    ${r.enabled ? '' : `<p class="routine-paused">${t(event ? 'Paused: incoming events are ignored. Run now still works.' : 'Paused: it will not run on schedule. Run now still works.')}</p>`}
    ${live ? `<p class="routine-live"><i class="live-dot"></i>${t(live.status === 'queued' ? 'Waiting for the Bot · since {time}' : 'Running now · started {time}', { time: when(live.started) })}</p>` : ''}
    <p class="detail-prompt">${escape(r.prompt)}</p>
    ${event ? '' : `<p class="muted">${t('Next {next} · last {last}', { next: r.enabled ? when(r.next) : '—', last: when(r.last) })}</p>`}
    ${event ? `<div class="routine-event"><code class="hook">${escape(r.webhook)}</code><p class="muted">${t('Keep this webhook URL private.')}</p><div class="routine-event-foot"><span class="event-evidence">${eventEvidence(r.eventStatus)}</span><button class="ghost" data-event-setup="${escape(r.id)}">${t('Set up and verify events')}</button></div></div>` : ''}
    <details class="routine-history" data-keep="history" ${live || runs.length ? 'open' : ''}><summary>${t('Run history')}${runs.length ? ` <span class="muted">${runs.length}</span>` : ''}</summary>${shown.length ? `<ol class="routine-runs">${shown.map(routineRunHtml).join('')}</ol>${runs.length > shown.length ? `<p class="muted">${t('Showing the latest {n}.', { n: shown.length })}</p>` : ''}` : `<p class="muted">${t('No runs yet.')}</p>`}</details>
    <div class="form-actions"><button class="danger" data-routine-delete="${escape(r.id)}">${t('Delete')}</button><span class="form-actions-end"><button class="ghost" data-routine-toggle="${escape(r.id)}" data-on="${r.enabled ? '0' : '1'}">${r.enabled ? t('Pause') : t('Resume')}</button><button class="ghost" data-routine-run="${escape(r.id)}" ${live ? 'disabled' : ''}>${t(live ? 'Running…' : 'Run now')}</button><button class="primary" data-routine-edit="${escape(r.id)}">${t('Edit')}</button></span></div>`
}
function routinesHtml() {
  const list = pageData.routines ?? []
  return `<div class="page">
    ${listHtml('routines', { items: () => pageData.routines ?? [], text: r => `${r.title} ${r.prompt} ${describeWhen(r.when)}`, card: routineCard, size: 12, placeholder: t('Search routines'), empty: query => t(query ? 'No matching routines.' : 'No routines yet.') }, `<span class="list-count muted">${t('{n} routines · {on} active', { n: list.length, on: list.filter(r => r.enabled).length })}</span><button class="primary list-add" data-routine-new>＋ ${t('New routine')}</button>`)}</div>`
}
// The create/edit form opens in a window; the same #rt-* fields are read when saving.
function routineFormHtml() {
  const list = pageData.routines ?? []
  const editing = list.find(r => r.id === routineEditing)
  const schedule = editing?.when ?? { kind: 'weekdays', time: '07:00' }
  const targets = routineTargets()
  return `<div class="routine-form">
    <h3 id="rt-heading">${t(editing ? 'Edit routine' : 'New routine')}</h3><div class="form-grid">
      <label>${t('Title')}<input id="rt-title" value="${escape(editing?.title ?? '')}" placeholder="${t('Morning briefing')}"></label>
      <label>${t('Send to')}<select id="rt-target">${targets.map(([k, n]) => `<option value="${escape(k)}" ${(editing?.target ?? routineTarget) === k ? 'selected' : ''}>${escape(n)}</option>`).join('')}</select></label>
      <label class="full">${t('What to do')}<textarea id="rt-prompt" rows="3" placeholder="${t('Check my calendar and email, and give me the three things that matter today.')}">${escape(editing?.prompt ?? '')}</textarea></label>
      <div class="full rt-schedule">
        <span class="rt-section-label">${t('Repeat')}</span>
        <input type="hidden" id="rt-kind" value="${escape(schedule.kind)}"><input type="hidden" id="rt-days" value="${escape(scheduleDays(schedule).join(','))}">
        <div class="seg rt-freq" role="radiogroup" aria-label="${t('Repeat')}">${ROUTINE_FREQS.map(([v, n]) => `<button type="button" role="radio" data-rt-freq="${v}">${t(n)}</button>`).join('')}</div>
        <label id="rt-more-label" hidden>${t('Trigger')}<select id="rt-more">${[['every', 'Every N minutes'], ['webhook', 'When a webhook is called'], ['event', 'Provider event']].map(([v, n]) => `<option value="${v}">${t(n)}</option>`).join('')}</select></label>
        <div id="rt-days-label" class="rt-days" role="group" aria-label="${t('Days')}" hidden>${[1, 2, 3, 4, 5, 6, 0].map(d => `<button type="button" data-rt-day="${d}" aria-pressed="false" title="${t(WEEKDAYS[d])}">${t(WEEKDAY_SHORT[d])}</button>`).join('')}<span class="rt-day-presets"><button type="button" class="link" data-rt-days="1,2,3,4,5">${t('Weekdays')}</button><button type="button" class="link" data-rt-days="0,1,2,3,4,5,6">${t('Every day')}</button></span></div>
        <div class="rt-when-row">
          <label id="rt-month-label" hidden>${t('Month')}<select id="rt-month">${Array.from({ length: 12 }, (_, i) => `<option value="${i + 1}" ${schedule.month === i + 1 ? 'selected' : ''}>${i + 1}${LANG === 'zh' ? '月' : ''}</option>`).join('')}</select></label>
          <label id="rt-mday-label" hidden>${t('Day of month')}<select id="rt-mday">${Array.from({ length: 31 }, (_, i) => `<option value="${i + 1}" ${(schedule.kind === 'monthly' || schedule.kind === 'yearly' ? schedule.day : 1) === i + 1 ? 'selected' : ''}>${i + 1}${LANG === 'zh' ? '日' : ''}</option>`).join('')}</select></label>
          <label id="rt-date-label" hidden>${t('Date')}<input id="rt-date" type="date"></label>
          <label id="rt-time-label">${t('Time')}<input id="rt-time" type="time" value="${escape(schedule.time ?? '07:00')}"></label>
          <label id="rt-minutes-label" hidden>${t('Every how many minutes')}<input id="rt-minutes" type="number" min="5" step="1" value="${escape(String(schedule.minutes ?? 60))}"></label>
          <label id="rt-zone-label">${t('Timezone')}<input id="rt-timezone" value="${escape(editing?.timezone ?? Intl.DateTimeFormat().resolvedOptions().timeZone ?? '')}" placeholder="Asia/Shanghai"></label>
        </div>
        <p id="rt-summary" class="rt-summary muted" aria-live="polite"></p>
      </div>
      <div id="rt-event-fields" class="full form-grid" hidden>
        <label>${t('Event source')}<select id="rt-source">${['github', 'slack', 'teams'].map(source => `<option ${schedule.source === source ? 'selected' : ''}>${source}</option>`).join('')}</select></label>
        <label>${t('Event type')}<input id="rt-event" value="${escape(schedule.event ?? '')}" placeholder="push / issues / message"></label>
        <label class="full">${t('Signing secret key name')}<input id="rt-signing" value="${escape(schedule.signingSecret ?? '')}" placeholder="GITHUB_WEBHOOK_SECRET"></label>
        <label class="full">${t('Event filters (JSON pointers)')}<textarea id="rt-filters" rows="2" placeholder='{"/ref":"refs/heads/main"}'>${escape(JSON.stringify(schedule.filters ?? {}))}</textarea></label>
      </div>
    </div><div class="form-actions"><button class="ghost" id="rt-cancel">${t('Cancel')}</button><button class="primary" id="rt-create">${t(editing ? 'Save' : 'Create routine')}</button></div></div>`
}
function openRoutineForm() { modal(routineFormHtml(), true); bindRoutineForm() }

let skillEditing = null, skillSaving = false
// The skill store index never changes while the server runs; install state comes from view.team.skills.
let skillStore = null, skillStoreLoading = null, storeCategory = 'all', storeCommand = '', storeCommandBusy = false
const storeInstalls = new Set()
const STORE_CATEGORIES = { official: 'Official', video: 'Video', commerce: 'Commerce & marketing', content: 'Content & publishing', slides: 'Presentations', audio: 'Voice & audio', documents: 'Documents & research', design: 'Design & visuals', dev: 'Development & automation', other: 'Other' }
function loadSkillStore() {
  skillStoreLoading ??= call('skill.store').then(store => { skillStore = store; if (view.page === 'skills') renderPage({ preserveDraft: true }) }).catch(error => { skillStoreLoading = null; toast(t(error.message)) })
}
function storeState(entry) {
  const skill = (view.team?.skills ?? []).find(s => s.name === entry.name)
  if (storeInstalls.has(entry.id)) return 'installing'
  if (!skill) return 'available'
  if (skill.origin?.source !== entry.id) return 'conflict'
  return skill.enabled === false ? 'disabled' : 'installed'
}
function skillStoreHtml() {
  if (!skillStore) { loadSkillStore(); return `<section class="skill-store"><h3>${t('Skill store')}</h3><p class="muted">${t('Loading…')}</p></section>` }
  const entries = skillStore.skills
  const used = new Set(entries.map(e => e.category))
  const chips = [['all', 'All'], ['common', 'Common'], ['installed', 'Installed'], ...Object.entries(STORE_CATEGORIES).filter(([key]) => used.has(key))]
  const blurb = SKILL_CATEGORIES[storeCategory] ? pick(SKILL_CATEGORIES[storeCategory].slice(2)) : ''
  return `<section class="skill-store"><div class="settings-section-title"><h3>${t('Skill store')}</h3><span>${t('{n} skills', { n: entries.length })}</span></div>
    <div class="store-filters">${chips.map(([key, name]) => `<button class="${storeCategory === key ? 'on' : ''}" data-store-category="${key}" aria-pressed="${storeCategory === key}">${SKILL_CATEGORIES[key] ? pick(SKILL_CATEGORIES[key]) : t(name)}</button>`).join('')}</div>
    <p class="plugin-blurb" id="store-blurb">${blurb}</p>
    ${listHtml('store', { items: storeItems, text: e => `${e.name} ${e.title ?? ''} ${e.description}`, card: storeCard, size: 12, placeholder: t('Search the skill store'), empty: () => t('No matching skills.') })}
    <p class="muted store-license">${t('Source: {repo} · upstream licenses apply', { repo: escape(skillStore.repository) })}</p></section>`
}
function skillCommandHtml() {
  return `<div class="store-command"><label for="store-command">${t('Install with a command')}</label><div class="store-command-row"><input id="store-command" spellcheck="false" autocomplete="off" value="${escape(storeCommand)}" placeholder="npx skills add owner/repo --skill name" ${storeCommandBusy ? 'disabled' : ''}><button class="primary" data-store-command ${storeCommandBusy ? 'disabled' : ''}>${t(storeCommandBusy ? 'Installing…' : 'Install')}</button></div></div>`
}
// The server only parses the pasted text for its source; nothing in it is executed.
const installStoreCommand = attempt(async () => {
  const command = ($('#store-command')?.value ?? storeCommand).trim()
  if (!command || storeCommandBusy) return
  storeCommand = command; storeCommandBusy = true; renderPage({ preserveDraft: true })
  try { const name = await call('skill.install', { command }); storeCommand = ''; if ($('#store-command')) $('#store-command').value = ''; await refresh(); toast(t('Installed {name}', { name })) }
  finally { storeCommandBusy = false; if (view.page === 'skills') renderPage({ preserveDraft: true }) }
})
const MARK_COLORS = ['#13b99a', '#ff8946', '#578bea', '#ef658d', '#9c79db', '#d7a25a', '#e85c65', '#36b8c9']
function markBadge(name) {
  const text = String(name || '?').trim(), hash = [...text].reduce((n, c) => (n * 31 + c.codePointAt(0)) >>> 0, 7)
  return `<span class="platform-badge" style="--platform-color:${MARK_COLORS[hash % MARK_COLORS.length]}">${escape([...text][0]?.toUpperCase() ?? '?')}</span>`
}
const storeItems = () => (skillStore?.skills ?? []).filter(e => storeCategory === 'all' || (storeCategory === 'common' ? e.common : storeCategory === 'installed' ? ['installed', 'disabled'].includes(storeState(e)) : e.category === storeCategory))
  // Common skills lead the list; the sort is stable, so the index order holds within each group.
  .sort((a, b) => Boolean(b.common) - Boolean(a.common))
function storeCard(e) {
  const label = { available: 'Install', installing: 'Installing…', installed: 'Installed', disabled: 'Installed (disabled)', conflict: 'Name in use' }
  const state = storeState(e)
  return `<article class="mini-card state-${state}"><span class="mini-icon">${markBadge(e.title || e.name)}</span><span class="mini-main"><strong title="${escape(e.title || e.name)}">${escape(e.title || e.name)}${e.common ? ` <span class="badge store-common">${t('Common')}</span>` : ''}</strong><small class="wrap" title="${escape(e.description)}">${escape(e.description)}</small><small>${SKILL_CATEGORIES[e.category] ? pick(SKILL_CATEGORIES[e.category]) : t('Other')} · <a href="${escape(e.url)}" target="_blank" rel="noopener noreferrer">${t('Source')}</a></small></span><span class="mini-side"><button class="${state === 'available' ? 'primary' : 'ghost'}" data-store-install="${escape(e.id)}" ${state === 'available' ? '' : 'disabled'}>${t(label[state])}</button></span></article>`
}
function skillOrigin(skill) {
  const origin = skill.origin ?? {}
  const label = t(origin.kind === 'imported' ? 'Imported workflow' : origin.kind === 'authored' ? 'Bot-authored workflow' : 'Local workflow')
  return `${label}${origin.by ? ` · ${botName(botById(origin.by)) || origin.by}` : ''}${origin.validation === 'untested' ? ` · ${t('Untested')}` : ''}`
}
function skillUsers(name) {
  const users = bots().filter(bot => (bot.skillProfile ?? []).some(s => s.name === name && s.loads > 0)).map(botName)
  return users.length ? t('Loaded by: {names}', { names: users.join('、') }) : t('Not loaded by a Bot yet')
}
// Plugins: one page for everything Bots can be given. Skills are written workflows; MCP connections are tool servers.
let pluginTab = 'installed', skillKind = 'all', mcpFilter = 'all', mcpCategory = 'all'
const PLUGIN_TABS = [['installed', 'Installed'], ['skills', 'Skill store'], ['mcp', 'MCP store'], ['manual', 'Manual install']]
const kindName = (table, key) => pick((table[key] ?? table.custom).slice(0, 2))
const kindHelp = (table, key) => pick((table[key] ?? table.custom).slice(2))
const skillCategory = skill => skillStore?.skills.find(e => e.name === skill.name)?.category ?? 'custom'
function mcpBadge(entry, name = '') {
  if (entry?.icon && ACCOUNT_BADGES[entry.icon]) return accountBadge(entry.icon)
  const mark = entry?.mark ?? (name || '?').slice(0, 1).toUpperCase()
  return `<span class="platform-badge ${mark.length > 1 ? 'mono' : ''}" style="--platform-color:${entry?.color ?? '#6b7280'}">${escape(mark)}</span>`
}
function kindChips(table, keys, current, attribute) {
  const chips = [['all', t('All')], ...keys.map(key => [key, kindName(table, key)])]
  return `<div class="store-filters">${chips.map(([key, name]) => `<button class="${current === key ? 'on' : ''}" ${attribute}="${key}" aria-pressed="${current === key}">${escape(name)}</button>`).join('')}</div>
    <p class="plugin-blurb">${current === 'all' ? '' : escape(kindHelp(table, current))}</p>`
}
function mcpCardHtml(c) {
  const entry = mcpEntryFor(c), category = entry?.category ?? 'custom'
  const target = c.kind === 'http' ? c.url : [c.command, ...c.args].join(' ')
  const use = entry ? pick(entry.use) : t('A custom connection. Its abilities are whatever that server offers; press Test connection to list them.')
  return `<div class="row-card mcp-card ${c.enabled ? '' : 'paused'}" data-mcp-card="${category}"><div class="account-head">${mcpBadge(entry, c.name)}<div class="grow"><div class="account-title"><strong>${escape(entry?.name ?? c.name)}</strong><span class="plugin-tag">${escape(kindName(MCP_CATEGORIES, category))}</span><span class="plugin-tag kind">${t(c.kind === 'http' ? 'Remote' : 'Local')}</span>${c.enabled ? '' : `<span class="badge">${t('Paused')}</span>`}</div>
      <p class="plugin-use">${escape(use)}</p><p class="muted mcp-target"><code>${escape(c.name)}</code> · ${escape(target)}</p>${c.check ? `<p class="connection-check ${c.check.state === 'error' ? 'error' : ''}">${t(c.check.state)} · ${when(c.check.checked)} · ${escape(c.check.error || `${(c.check.tools ?? []).length} ${t('available tools')}`)}</p>` : `<p class="muted">${t('Not tested')}</p>`}</div></div>
    <div class="account-foot"><span></span><div class="row-actions"><button class="ghost" data-connection-test="${escape(c.name)}">${t('Test connection')}</button><button class="ghost" data-connection-edit="${escape(c.name)}">${t('Edit')}</button><button class="ghost" data-connection-toggle="${escape(c.name)}">${c.enabled ? t('Pause') : t('Resume')}</button><button class="danger" data-connection-delete="${escape(c.name)}">${t('Remove')}</button></div></div></div>`
}
const connectionStatus = c => !c.enabled ? ['Paused', 'warn'] : c.check?.state === 'error' ? ['error', 'err'] : c.check?.state === 'connected' ? ['connected', 'on'] : ['Not tested', '']
function mcpMiniCard(c) {
  const entry = mcpEntryFor(c), category = entry?.category ?? 'custom', [status, tone] = connectionStatus(c)
  return `<button class="mini-card ${c.enabled ? '' : 'is-off'}" data-detail="mcp:${escape(c.name)}"><span class="mini-icon">${mcpBadge(entry, c.name)}</span><span class="mini-main"><strong>${escape(entry?.name ?? c.name)}</strong><small>${escape(kindName(MCP_CATEGORIES, category))} · ${t(c.kind === 'http' ? 'Remote' : 'Local')}</small></span><span class="mini-status ${tone}">${t(status)}</span></button>`
}
DETAILS.mcp = name => {
  const c = (view.team?.connections ?? []).find(x => x.name === name)
  return c ? mcpCardHtml(c) : ''
}
function skillMiniCard(s) {
  return `<button class="mini-card ${s.enabled === false ? 'is-off' : ''}" data-detail="skill:${escape(s.name)}"><span class="mini-icon">${markBadge(s.title || s.name)}</span><span class="mini-main"><strong>${escape(s.title || s.name)}</strong><small class="wrap">${escape(s.description)}</small></span><span class="mini-status ${s.enabled === false ? 'warn' : 'on'}">${t(s.enabled === false ? 'Skill disabled' : 'Skill enabled')}</span></button>`
}
DETAILS.skill = name => {
  const s = (view.team?.skills ?? []).find(x => x.name === name)
  if (!s) return ''
  return `<div class="detail-head"><h3>${escape(s.title || s.name)}</h3><span class="badge">${t(s.enabled === false ? 'Skill disabled' : 'Skill enabled')}</span></div>
    <p class="muted"><b>${t('Used for')}</b> ${escape(s.description)}</p><p class="muted">${escape(s.name)} · ${t('Shared with all Bots')} · ${escape(skillOrigin(s))}</p>${s.quality?.reason ? `<p class="error">${escape(s.quality.reason)}</p>` : ''}${s.bundleError ? `<p class="error">${escape(s.bundleError)}</p>` : ''}<p class="muted">${escape(skillUsers(s.name))} · ${skillQuality(s.name)}</p>${s.origin?.url ? `<a href="${escape(s.origin.url)}" target="_blank" rel="noopener noreferrer">${t('Skill source')}</a>` : ''}
    <details class="skill-body" data-keep="body"><summary>${t('Steps (Markdown)')}</summary><pre>${escape(s.body ?? s.text)}</pre></details>
    <div class="form-actions detail-actions"><button class="ghost" data-skill-toggle="${escape(s.name)}">${t(s.enabled === false ? 'Enable skill' : 'Disable skill')}</button><button class="ghost" data-skill-validate="${escape(s.name)}">校验技能包</button><button class="ghost" data-skill-versions="${escape(s.name)}">版本与回滚</button><button class="ghost" data-skill-candidates="${escape(s.name)}">候选改进</button><button class="ghost" data-skill-trial="${escape(s.name)}" ${s.enabled === false ? 'disabled' : ''}>试运行与评估</button><button class="ghost" data-skill-edit="${escape(s.name)}">${t('Edit')}</button><button class="danger" data-skill-delete="${escape(s.name)}">${t('Delete')}</button></div>`
}
function pluginsInstalledHtml() {
  if (!skillStore) loadSkillStore()
  if (!mcpDirectory) loadMcpDirectory()
  const connections = view.team?.connections ?? [], skills = view.team?.skills ?? []
  const mcpKinds = [...new Set(connections.map(c => mcpEntryFor(c)?.category ?? 'custom'))]
  const skillKinds = [...new Set(skills.map(skillCategory))]
  if (!mcpKinds.includes(mcpFilter)) mcpFilter = 'all'
  if (!skillKinds.includes(skillKind)) skillKind = 'all'
  return `<section class="conn-section"><div class="settings-section-title"><h3>${t('MCP connections')}</h3><span>${connections.length}</span></div>
    ${connections.length ? kindChips(MCP_CATEGORIES, mcpKinds, mcpFilter, 'data-mcp-filter') : ''}
    ${listHtml('mcp-installed', { items: () => (view.team?.connections ?? []).filter(c => mcpFilter === 'all' || (mcpEntryFor(c)?.category ?? 'custom') === mcpFilter), text: c => `${c.name} ${mcpEntryFor(c)?.name ?? ''} ${kindName(MCP_CATEGORIES, mcpEntryFor(c)?.category ?? 'custom')}`, card: mcpMiniCard, size: 8, placeholder: t('Search connections'), empty: query => t(query || connections.length ? 'No matching connections.' : 'No connections yet. Add one from the MCP store.') })}</section>
    <section class="conn-section"><div class="settings-section-title"><h3>${t('Skills')}</h3><span>${skills.length}</span></div>
    <label class="learning-toggle" title="${t('Bots already save reusable workflows and lessons while they work, and search past conversations when needed. Turning this on also reviews every finished task with an extra model call and tests skill changes with paired runs: more thorough, but costly. Off by default.')}"><input id="learning-enabled" type="checkbox" ${view.team?.prefs.learningEnabled === true ? 'checked' : ''}><span>${t('Curator')}</span></label>
    ${skills.length ? kindChips(SKILL_CATEGORIES, skillKinds, skillKind, 'data-skill-kind') : ''}
    ${listHtml('skills-installed', { items: () => (view.team?.skills ?? []).filter(s => skillKind === 'all' || skillCategory(s) === skillKind), text: s => `${s.name} ${s.title ?? ''} ${t(s.title || '')} ${s.description}`, card: skillMiniCard, size: 12, placeholder: t('Search skills'), empty: query => t(query || skills.length ? 'No matching skills.' : 'No skills yet.') })}</section>`
}
// The store: the 300 most viewed servers from ModelScope's MCP 广场, a domestic marketplace with
// Chinese names and descriptions. Each carries its install command and the settings it needs, so
// adding one never needs a README.
let mcpDirectory = null, mcpDirectoryLoading = null
function loadMcpDirectory() {
  mcpDirectoryLoading ??= call('mcp.store').then(index => { mcpDirectory = index; if (view.page === 'skills' && ['mcp', 'installed'].includes(pluginTab)) renderPage({ preserveDraft: true }) }).catch(error => { mcpDirectoryLoading = null; toast(t(error.message)) })
}
const mcpServers = () => mcpDirectory?.servers ?? []
const mcpServerAdded = server => (view.team?.connections ?? []).some(c => mcpConnectionKey(c) === mcpServerKey(server))
const mcpNeeds = server => server.env.filter(e => e.required)
const mcpItems = () => mcpServers().filter(server => mcpCategory === 'all' || (mcpCategory === 'added' ? mcpServerAdded(server) : server.cat === mcpCategory))
const mcpQuote = word => (/\s/.test(word) ? JSON.stringify(word) : word)
const mcpCommandLine = server => server.kind === 'http' ? server.url : [server.command, ...server.args].map(mcpQuote).join(' ')
const viewsText = n => (n >= 10000 ? `${(n / 10000).toFixed(n >= 100000 ? 0 : 1)} 万` : String(n))
function mcpStoreCard(server) {
  const added = mcpServerAdded(server), needs = mcpNeeds(server).length
  return `<article class="mini-card ${added ? 'state-installed' : ''}"><button class="mini-body" data-detail="mcpstore:${escape(server.id)}"><span class="mini-icon">${markBadge(server.name)}</span><span class="mini-main"><strong title="${escape(server.name)}">${escape(server.name)}${server.verified ? ` <span class="badge store-common">${t('Verified')}</span>` : ''}</strong><small class="wrap" title="${escape(server.desc)}">${escape(server.desc)}</small><small>${escape(kindName(MCP_CATEGORIES, server.cat))} · ${t(server.kind === 'http' ? 'Remote' : 'Local')}${needs ? ` · ${t('Needs a key')}` : ''}</small></span></button><span class="mini-side"><button class="${added ? 'ghost' : 'primary'}" data-mcp-add="${escape(server.id)}" ${added ? 'disabled' : ''}>${t(added ? 'Added' : 'Add')}</button></span></article>`
}
DETAILS.mcpstore = id => {
  const server = mcpServers().find(x => x.id === id)
  if (!server) return ''
  const added = mcpServerAdded(server), tool = server.kind === 'http' ? '' : server.command === 'npx' ? 'Node.js（npx）' : 'uv（uvx）'
  return `<div class="detail-head"><span class="mini-icon">${markBadge(server.name)}</span><h3>${escape(server.name)}</h3>${server.verified ? `<span class="badge store-common">${t('Verified')}</span>` : ''}</div>
    <p class="detail-prompt">${escape(server.desc)}</p>
    <dl class="detail-facts"><div><dt>${t('Category')}</dt><dd>${escape(kindName(MCP_CATEGORIES, server.cat))}</dd></div><div><dt>${t('Runs as')}</dt><dd>${t(server.kind === 'http' ? 'Remote' : 'Local')}</dd></div>${server.author ? `<div><dt>${t('Author')}</dt><dd>${escape(server.author)}</dd></div>` : ''}<div><dt>${t('Popularity')}</dt><dd>${t('{n} views', { n: viewsText(server.views) })}</dd></div></dl>
    <h4 class="detail-title">${t(server.kind === 'http' ? 'Address' : 'Command it runs')}</h4><pre class="cmd">${escape(mcpCommandLine(server))}</pre>
    ${tool ? `<p class="muted detail-note">${t('The computer running dotbot needs {tool}.', { tool })}</p>` : ''}
    <h4 class="detail-title">${t('Settings it needs')}</h4>
    ${server.env.length ? `<ul class="env-list">${server.env.map(e => `<li><code>${escape(e.name)}</code><span class="tag">${t(e.required ? 'Required' : 'Optional')}</span>${e.desc ? `<small>${escape(e.desc)}</small>` : ''}</li>`).join('')}</ul>` : `<p class="muted detail-note">${t('No key or setting needed: add it and it works.')}</p>`}
    <p class="detail-links"><a href="${escape(server.page)}" target="_blank" rel="noopener noreferrer">${t('Open on ModelScope')}</a>${server.source && server.source !== server.page ? ` · <a href="${escape(server.source)}" target="_blank" rel="noopener noreferrer">${t('Project page')}</a>` : ''}</p>
    <div class="form-actions"><button class="ghost" data-close>${t('Close')}</button><button class="${added ? 'ghost' : 'primary'}" data-mcp-add="${escape(server.id)}" ${added ? 'disabled' : ''}>${t(added ? 'Added' : 'Add')}</button></div>`
}
function mcpStoreHtml() {
  if (!mcpDirectory) loadMcpDirectory()
  const servers = mcpServers(), counts = {}
  for (const server of servers) counts[server.cat] = (counts[server.cat] ?? 0) + 1
  const added = servers.filter(mcpServerAdded).length
  const chips = [['all', t('All'), servers.length], ['added', t('Added'), added], ...(mcpDirectory?.categories ?? []).map(c => [c.id, kindName(MCP_CATEGORIES, c.id), counts[c.id] ?? 0])]
  return `<section class="skill-store"><div class="settings-section-title"><h3>${t('MCP store')}</h3><span>${t('{n} servers', { n: servers.length })} · ${t('from')} <a href="${escape(mcpDirectory?.url ?? 'https://www.modelscope.cn/mcp')}" target="_blank" rel="noopener noreferrer">${t('ModelScope MCP Square')}</a></span></div>
    <div class="store-filters">${chips.map(([key, name, count]) => `<button class="${mcpCategory === key ? 'on' : ''}" data-mcp-category="${key}" aria-pressed="${mcpCategory === key}">${escape(name)} <span class="chip-count">${count}</span></button>`).join('')}</div>
    <p class="plugin-blurb">${MCP_CATEGORIES[mcpCategory] ? escape(kindHelp(MCP_CATEGORIES, mcpCategory)) : mcpCategory === 'added' ? t('Servers you have already added from this store.') : t('The most used servers in China, with Chinese names and one-click setup.')}</p>
    ${listHtml('mcp-store', { items: mcpItems, text: server => `${server.name} ${server.desc} ${server.author} ${kindName(MCP_CATEGORIES, server.cat)}`, card: mcpStoreCard, size: 12, placeholder: t('Search the MCP store'), empty: () => mcpDirectory ? t(mcpCategory === 'added' ? 'Nothing added from the store yet.' : 'No matching servers.') : t('Loading…') })}
    <p class="muted store-license">${t('Listings come from ModelScope MCP Square; each server is its authors\' own work under its own license.')}</p></section>`
}
// A connection's name is a short word: the package's last path part without "mcp" and "server".
function mcpConnectionName(server) {
  const taken = new Set((view.team?.connections ?? []).map(c => c.name))
  const pkg = server.kind === 'http' ? new URL(server.url).hostname.split('.').slice(-2)[0] : (server.args.find(a => !a.startsWith('-')) ?? server.id).replace(/(?<=.)@[\w.\-]+$/, '').split('/').pop()
  let base = pkg.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/(^|-)(mcp|server)(?=-|$)/g, '$1').replace(/-+/g, '-').replace(/^-+|-+$/g, '').slice(0, 26) || 'mcp'
  let name = base
  for (let n = 2; taken.has(name); n++) name = `${base}-${n}`
  return name
}
const MCP_SECRET = /KEY|TOKEN|SECRET|PASS|PWD|COOKIE|AUTH|CREDENTIAL/i
const MCP_PLACEHOLDER = /<[^>]+>|your[-_ ]|path\/to|\/path\/|xxx|\.\.\./i
// Adding from the store: only what cannot be guessed is asked for, and the exact command is shown first.
function mcpAddModal(id) {
  const server = mcpServers().find(x => x.id === id)
  if (!server) return
  const saved = new Set(view.team?.secrets ?? []), name = mcpConnectionName(server)
  const holes = server.kind === 'stdio' ? server.args.map((a, i) => (MCP_PLACEHOLDER.test(a) ? i : -1)).filter(i => i >= 0) : []
  modal(`<button class="icon detail-close" data-close aria-label="${t('Close')}" title="${t('Close')}">✕</button><div class="detail-head"><span class="mini-icon">${markBadge(server.name)}</span><h3>${t('Add {name}', { name: escape(server.name) })}</h3></div>
    <p class="muted">${escape(server.desc)}</p>
    <div class="cmd-preview"><small>${t(server.kind === 'http' ? 'Connects to' : 'It will run')}</small><code>${escape(mcpCommandLine(server))}</code></div>
    <div class="form-grid mcp-add-form">
    ${holes.map(i => `<label class="full">${t('Replace the placeholder with a real value')}<input id="mi-arg-${i}" value="${escape(server.args[i])}" spellcheck="false" autocomplete="off"></label>`).join('')}
    ${server.env.map((e, n) => {
      const secret = MCP_SECRET.test(e.name), label = `<code>${escape(e.name)}</code>${e.required ? '' : ` <span class="muted">${t('Optional')}</span>`}`, hint = e.desc ? `<small class="muted">${escape(e.desc)}</small>` : ''
      if (secret && saved.has(e.name)) return `<p class="full muted">✓ ${t('Key {name} is already saved and will be used.', { name: `<code>${escape(e.name)}</code>` })}</p>`
      return `<label class="full">${label}<input id="mi-env-${n}" ${secret ? 'type="password"' : 'type="text"'} autocomplete="off" spellcheck="false" placeholder="${escape(secret ? t('Stays on this server') : '')}">${hint}</label>`
    }).join('')}
    </div>
    <p class="muted mcp-trust">${t('Adding it runs this on the computer running dotbot and gives every Bot its tools. Only add servers you trust.')}</p>
    <div class="form-actions"><button class="ghost" data-close>${t('Cancel')}</button><button class="primary" id="mi-save">${t('Add')}</button></div>`)
  $('#mi-save').onclick = attempt(async () => {
    const args = [...server.args], env = {}
    for (const i of holes) {
      const value = $(`#mi-arg-${i}`).value.trim()
      if (!value || MCP_PLACEHOLDER.test(value)) throw new Error(t('Replace “{text}” with a real value.', { text: server.args[i] }))
      args[i] = value
    }
    for (const [n, e] of server.env.entries()) {
      const secret = MCP_SECRET.test(e.name)
      if (secret && saved.has(e.name)) { env[e.name] = '${' + e.name + '}'; continue }
      const value = $(`#mi-env-${n}`).value.trim()
      if (!value) { if (e.required) throw new Error(t('Enter a value for {name}.', { name: e.name })); continue }
      env[e.name] = secret ? '${' + await call('secret.set', { name: e.name, value }) + '}' : value
    }
    await call('connection.save', { name, kind: server.kind, ...(server.kind === 'http' ? { url: server.url, headers: env } : { command: server.command, args, env }), enabled: true })
    closeModal(); await refresh(); pluginTab = 'installed'; mcpFilter = 'all'; resetList('mcp-installed'); renderPage()
    toast(t('Added. The first run downloads the server; press Test connection under Installed to check it.'))
  })
}
// Reads a configuration pasted from a README: an mcpServers JSON block, a single server object,
// an npx/uvx command line, or `claude mcp add …`.
function parseMcpConfig(text) {
  const source = text.trim().replace(/^```\w*\s*|\s*```$/g, '').trim()
  if (!source) throw new Error(t('Paste a configuration first.'))
  const fromObject = (name, config) => {
    if (typeof config?.url === 'string') return { name, kind: 'http', target: config.url, pairs: pairsText(config.headers) }
    if (typeof config?.command === 'string') return { name, kind: 'stdio', target: [config.command, ...(config.args ?? [])].map(mcpQuote).join(' '), pairs: pairsText(config.env) }
    throw new Error(t('That configuration has no command or address.'))
  }
  if (source.startsWith('{')) {
    let value
    try { value = JSON.parse(source) } catch { throw new Error(t('That is not valid JSON.')) }
    const servers = value.mcpServers ?? value.servers
    if (servers && typeof servers === 'object') { const [name, config] = Object.entries(servers)[0] ?? []; return fromObject(name, config) }
    return fromObject('', value)
  }
  const words = [...source.matchAll(/"([^"]*)"|'([^']*)'|(\S+)/g)].map(m => m[1] ?? m[2] ?? m[3])
  if (words[0] === 'claude' && words[1] === 'mcp' && words[2] === 'add') {
    const rest = words.slice(3), flags = {}
    while (rest[0]?.startsWith('-')) { const flag = rest.shift(); if (['--transport', '-t', '--env', '-e', '--header', '-H', '--scope', '-s'].includes(flag)) (flags[flag] ??= []).push(rest.shift()) }
    const name = rest.shift() ?? '', transport = (flags['--transport'] ?? flags['-t'] ?? [])[0]
    if (rest[0] === '--') rest.shift()
    const pairsList = [...(flags['--env'] ?? []), ...(flags['-e'] ?? []), ...(flags['--header'] ?? []), ...(flags['-H'] ?? [])].map(p => p.replace(/^([^=:]+)[=:]\s*/, '$1=')).join('\n')
    return /^https?:\/\//.test(rest[0] ?? '') || transport === 'http' ? { name, kind: 'http', target: rest[0] ?? '', pairs: pairsList } : { name, kind: 'stdio', target: rest.map(mcpQuote).join(' '), pairs: pairsList }
  }
  if (/^https?:\/\//.test(words[0])) return { name: '', kind: 'http', target: words[0], pairs: '' }
  if (['npx', 'uvx', 'node', 'python', 'python3', 'docker', 'bunx', 'pipx', 'deno'].includes(words[0])) return { name: '', kind: 'stdio', target: words.map(mcpQuote).join(' '), pairs: '' }
  throw new Error(t('Could not recognise that. Paste the JSON block or the command from the README.'))
}
function pluginsManualHtml() {
  const edit = skillEditing, connections = view.team?.connections ?? []
  return `<section class="conn-section"><h3 id="cn-heading">${connectionEditing ? t('Edit {name}', { name: escape(connectionEditing) }) : t('Add an MCP connection by hand')}</h3>
    ${connectionEditing ? '' : `<div class="mcp-paste"><label for="cn-paste">${t('Paste a configuration from the README')}</label><textarea id="cn-paste" rows="3" spellcheck="false" placeholder='{"mcpServers": {"name": {"command": "npx", "args": ["-y", "some-mcp-server"], "env": {"API_KEY": "..."}}}}   ·   npx -y some-mcp-server   ·   claude mcp add …'></textarea><div class="form-actions"><span class="muted">${t('Accepts the JSON block, a command line or claude mcp add. It fills the fields below.')}</span><button class="ghost" id="cn-paste-go">${t('Recognise and fill in')}</button></div></div>`}<div class="form-grid">
      <label>${t('Name')}<input id="cn-name" placeholder="github"></label>
      <label>${t('Kind')}<select id="cn-kind"><option value="http">${t('Remote (an address)')}</option><option value="stdio">${t('Local (a command)')}</option></select></label>
      <label class="full">${t('Address, or command and arguments')}<input id="cn-target" placeholder="${t('https://api.example.com/mcp   or   npx -y some-mcp-server')}"></label>
      <label class="full">${t('Headers (remote) or environment (local), one NAME=value per line')}<textarea id="cn-pairs" rows="3" placeholder="Authorization=Bearer \${GITHUB_TOKEN}"></textarea></label>
    </div><div class="form-actions">${connectionEditing ? `<button class="ghost" id="cn-cancel">${t('Cancel')}</button>` : '<span></span>'}<button class="primary" id="cn-save">${connectionEditing ? t('Save') : t('Connect')}</button></div></section>
    <section class="conn-section"><h3>${t('Install a skill from a source')}</h3>${skillCommandHtml()}</section>
    <section class="conn-section"><h3 id="sk-heading">${t(edit ? 'Edit skill' : 'New skill')}${edit ? ` · ${escape(edit.title || edit.name)}` : ''}</h3><div class="form-grid">
      <label>${t('Name')}<input id="sk-name" placeholder="weekly-report" value="${escape(edit?.name || '')}" ${edit ? 'readonly' : ''} ${skillSaving ? 'disabled' : ''}></label>
      <label>${t('When to use it')}<input id="sk-desc" value="${escape(edit?.description || '')}" placeholder="${t('Use when the user asks for the weekly report')}" ${skillSaving ? 'disabled' : ''}></label>
      <label class="full">${t('Steps (Markdown)')}<textarea id="sk-text" rows="10" ${skillSaving ? 'disabled' : ''}>${escape(edit?.body || '')}</textarea><p class="muted field-hint">${t('Never store passwords or tokens.')}</p></label>
    </div><div class="form-actions"><button class="ghost" id="sk-template" ${skillSaving ? 'disabled' : ''}>${t('Insert workflow template')}</button>${edit ? `<button class="ghost" id="sk-cancel" ${skillSaving ? 'disabled' : ''}>${t('Cancel')}</button>` : ''}<button class="primary" id="sk-save" ${skillSaving ? 'disabled' : ''}>${t(skillSaving ? 'Saving…' : 'Save skill')}</button></div></section>`
}
function skillsHtml() {
  const body = { installed: pluginsInstalledHtml, skills: skillStoreHtml, mcp: mcpStoreHtml, manual: pluginsManualHtml }[pluginTab]()
  return `<div class="page skills-page plugins-page">
    <div class="plugin-tabs" role="tablist">${PLUGIN_TABS.map(([key, name]) => `<button role="tab" class="${pluginTab === key ? 'on' : ''}" aria-selected="${pluginTab === key}" data-plugin-tab="${key}">${t(name)}</button>`).join('')}</div>${body}</div>`
}
async function saveSkill() {
  if (skillSaving) return
  const params = { name: $('#sk-name').value, description: $('#sk-desc').value, text: $('#sk-text').value, revision: skillEditing?.revision ?? '' }
  skillSaving = true
  for (const el of document.querySelectorAll('#sk-name, #sk-desc, #sk-text, #sk-save, #sk-template, #sk-cancel')) el.disabled = true
  let saved = false
  try {
    const name = await call('skill.save', params)
    saved = true; skillEditing = null; pluginTab = 'installed'
    toast(t('Skill saved as {name}', { name }))
  } finally {
    skillSaving = false
    // The fields were disabled in the DOM, not in the markup: force the redraw so they come back.
    if (view.page === 'skills') { renderedMarkup.delete($('#settings-content')); renderPage({ preserveDraft: !saved }) }
  }
}

// Account marks (Simple Icons / Primer Octicons / selfh.st; trademarks of their owners, used only to name the service) and brand colors.
const ACCOUNT_BADGES = {
  google: ['G', '#4285f4'],
  gmail: ['<svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><path d="M24 5.457v13.909c0 .904-.732 1.636-1.636 1.636h-3.819V11.73L12 16.64l-6.545-4.91v9.273H1.636A1.636 1.636 0 0 1 0 19.366V5.457c0-2.023 2.309-3.178 3.927-1.964L5.455 4.64 12 9.548l6.545-4.91 1.528-1.145C21.69 2.28 24 3.434 24 5.457z"/></svg>', '#ea4335'],
  drive: ['<svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><path d="M12.01 1.485c-2.082 0-3.754.02-3.743.047.01.02 1.708 3.001 3.774 6.62l3.76 6.574h3.76c2.081 0 3.753-.02 3.742-.047-.005-.02-1.708-3.001-3.775-6.62l-3.76-6.574zm-4.76 1.73a789.828 789.861 0 0 0-3.63 6.319L0 15.868l1.89 3.298 1.885 3.297 3.62-6.335 3.618-6.33-1.88-3.287C8.1 4.704 7.255 3.22 7.25 3.214zm2.259 12.653-.203.348c-.114.198-.96 1.672-1.88 3.287a423.93 423.948 0 0 1-1.698 2.97c-.01.026 3.24.042 7.222.042h7.244l1.796-3.157c.992-1.734 1.85-3.23 1.906-3.323l.104-.167h-7.249z"/></svg>', '#4285f4'],
  github: ['<svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><path d="M10.226 17.284c-2.965-.36-5.054-2.493-5.054-5.256 0-1.123.404-2.336 1.078-3.144-.292-.741-.247-2.314.09-2.965.898-.112 2.111.36 2.83 1.01.853-.269 1.752-.404 2.853-.404 1.1 0 1.999.135 2.807.382.696-.629 1.932-1.1 2.83-.988.315.606.36 2.179.067 2.942.72.854 1.101 2 1.101 3.167 0 2.763-2.089 4.852-5.098 5.234.763.494 1.28 1.572 1.28 2.807v2.336c0 .674.561 1.056 1.235.786 4.066-1.55 7.255-5.615 7.255-10.646C23.5 6.188 18.334 1 11.978 1 5.62 1 .5 6.188.5 12.545c0 4.986 3.167 9.12 7.435 10.669.606.225 1.19-.18 1.19-.786V20.63a2.9 2.9 0 0 1-1.078.224c-1.483 0-2.359-.808-2.987-2.313-.247-.607-.517-.966-1.034-1.033-.27-.023-.359-.135-.359-.27 0-.27.45-.471.898-.471.652 0 1.213.404 1.797 1.235.45.651.921.943 1.483.943.561 0 .92-.202 1.437-.719.382-.381.674-.718.944-.943"/></svg>', 'var(--text)'],
  microsoft: ['<svg viewBox="0 0 512 512" fill="currentColor" aria-hidden="true"><path d="M75.9 417.3c-.9-.4-1.7-.8-2.6-1.2h-.1c-.8-.4-1.7-.8-2.5-1.3h-.1l-3.8-2.3c-26.4-16-42.8-45.1-42.8-76v-161c0-30.9 16.4-60 42.8-76.1l100-60.5c-8.4 15-13 32.2-13 50v109.4l-.1.1v51.4c0 35.6 18.9 69.1 49.3 87.4l40 24.2l-84.2 51c-.8.5-1.7 1-2.5 1.5c-.7.4-1.4.7-2 1.1c-.2.1-.4.2-.5.3c-24.3 12.5-53.1 13.2-77.9 2M488 338.9q0-.15 0 0c0-1-.1-1.9-.2-2.8c-2-25.9-15-49.5-35.7-64.7c0 0-.1 0-.1-.1l-2.1-1.5l-.9-.6c-.5-.3-.9-.6-1.4-.9l-2.4-1.5l-87-52.7v35.7c0 35.6-18.9 69.1-49.3 87.4l-40.1 24.3l-103.1 62.4c-1 .6-2.1 1.2-3.1 1.8c-.5.3-1 .5-1.5.8c-.6.3-1.1.6-1.7.9c-.7.3-1.3.7-2 1l-1.2.6c-.8.4-1.6.7-2.3 1.1c-.3.1-.6.3-.9.4c-.9.4-1.8.7-2.7 1.1c-.2.1-.4.1-.5.2c-1 .4-2 .7-3 1.1c-.1 0-.2.1-.3.1c-1.1.4-2.1.7-3.2 1.1c-10.8 3.4-22.1 5-33.4 4.7L210 499.1c28.4 17.2 63.6 17.2 92 0l143.2-86.8c25.9-15.7 41.9-43.1 42.8-73.4M167.2 86.2v104.2l35.9-21.8c16.3-9.9 34.6-14.8 52.9-14.8s36.6 4.9 52.9 14.8l143.2 86.7c.9.6 1.9 1.1 2.8 1.7c.4.3.8.6 1.3.8c.5.3.9.6 1.4 1s1.1.8 1.6 1.1c.3.2.7.5 1 .7c.6.4 1.2.9 1.8 1.3c.3.2.5.4.8.6c.6.5 1.3 1 1.9 1.5c.2.2.4.3.6.5c.7.6 1.3 1.1 2 1.7c.1.1.3.2.4.4c.7.6 1.4 1.2 2 1.9c.1.1.2.2.3.2c.7.7 1.4 1.3 2.1 2l.1.1c.7.7 1.4 1.5 2.1 2.2c5.4 5.8 10.1 12.2 14 19.1V175.5c-.1-30.9-16.6-60.1-42.9-76.1L301.9 12.8c-27.2-16.5-61.4-17.1-89.2-1.6l-2.8 1.7c-25.5 15.5-41.7 43.2-42.8 72.9c.1.1.1.2.1.4"/></svg>', '#0078d4'],
}
const accountBadge = kind => { const [mark, color] = ACCOUNT_BADGES[kind] ?? ['?', '#888']; return `<span class="platform-badge" style="--platform-color:${color}">${mark}</span>` }

const ACCOUNT_STATUS = { disconnected: 'Not signed in', authorized: 'Signed in', connected: 'Connected', consent: 'Sign in to add permissions', expired: 'Sign-in expired', error: 'Connection error' }
const ACCESS_TEXT = { off: 'Off', read: 'Read', write: 'Read & write' }
function accountCardHtml(a) {
  const catalog = view.team?.accountCatalog ?? []
  const supported = catalog.some(p => p.id === a.provider), signedIn = ['authorized', 'connected', 'consent'].includes(a.status)
  const services = (a.services ?? []).map(s => `<span class="service-chip access-${escape(s.access)}">${escape(t(s.name))}<b>${t(ACCESS_TEXT[s.access])}</b></span>`).join('')
  const signIn = a.enabled && (!signedIn || a.status === 'consent') ? `<button class="primary" data-account-authorize="${escape(a.id)}">${t(a.status === 'disconnected' ? 'Sign in' : 'Sign in again')}</button>` : ''
  const line = [a.identity, supported ? '' : t('Older single-service connection'), a.provider === 'google' ? '' : t(a.writable ? 'Writes enabled' : 'Read access')].filter(Boolean).join(' · ')
  return `<div class="row-card account-card ${a.enabled ? '' : 'paused'}"><div class="account-head">${accountBadge(a.provider)}<div class="grow"><div class="account-title"><strong>${escape(a.name)}</strong><span class="badge status-${escape(a.status)}">${t(!a.enabled ? 'Paused' : ACCOUNT_STATUS[a.status] ?? a.status)}</span></div>${line ? `<p class="muted">${escape(line)}</p>` : ''}</div></div>
        ${services ? `<div class="service-chips">${services}</div>` : `<details class="account-scopes"><summary>${t('Granted permissions')}</summary><ul class="scope-list">${(a.grantedScopes.length ? a.grantedScopes : a.scopes).map(s => `<li>${escape(s)}</li>`).join('')}</ul></details>`}${a.error ? `<p class="error account-error">${escape(t(a.error))}</p>` : ''}
        <div class="account-foot"><small class="muted">${t('Last tested')} ${when(a.checked)}</small><div class="row-actions">${signIn}${supported ? `<button class="ghost" data-account-edit="${escape(a.id)}">${t('Permissions')}</button>` : ''}<button class="ghost" data-account-test="${escape(a.id)}">${t('Test connection')}</button><button class="ghost" data-account-toggle="${escape(a.id)}">${t(a.enabled ? 'Pause' : 'Resume')}</button><span class="account-danger"><button class="danger" data-account-revoke="${escape(a.id)}">${t('Revoke access')}</button><button class="danger" data-account-delete="${escape(a.id)}">${t('Remove')}</button></span></div></div></div>`
}
const accountStatus = a => !a.enabled ? ['Paused', 'warn'] : a.status === 'error' || a.status === 'expired' ? [ACCOUNT_STATUS[a.status], 'err'] : a.status === 'consent' || a.status === 'disconnected' ? [ACCOUNT_STATUS[a.status], 'warn'] : [ACCOUNT_STATUS[a.status] ?? a.status, 'on']
function accountMiniCard(a) {
  const [status, tone] = accountStatus(a), services = (a.services ?? []).filter(s => s.access !== 'off').map(s => t(s.name))
  return `<button class="mini-card ${a.enabled ? '' : 'is-off'}" data-detail="account:${escape(a.id)}"><span class="mini-icon">${accountBadge(a.provider)}</span><span class="mini-main"><strong>${escape(a.name)}</strong><small>${escape([a.identity, services.join('、')].filter(Boolean).join(' · ') || t(a.writable ? 'Writes enabled' : 'Read access'))}</small></span><span class="mini-status ${tone}">${t(status)}</span></button>`
}
DETAILS.account = id => {
  const a = (view.team?.accounts ?? []).find(x => x.id === id)
  return a ? accountCardHtml(a) : ''
}
// Sign-in services as store cards, the same shape as the Plugins stores.
function accountStoreCard(p) {
  const n = (view.team?.accounts ?? []).filter(a => a.provider === p.id).length, ready = view.team?.accountApps?.[p.id]?.configured
  const use = ACCOUNT_USE[p.id] ? pick(ACCOUNT_USE[p.id]) : (p.services ?? []).map(s => t(s.name)).join('、')
  return `<article class="mini-card ${n ? 'state-installed' : ''}"><span class="mini-icon">${accountBadge(p.id)}</span><span class="mini-main"><strong>${escape(p.name)}${n ? ` <span class="badge store-common">${t('{n} connected', { n })}</span>` : ''}</strong><small class="wrap">${escape(use)}</small><small><button class="link-button" data-account-app="${escape(p.id)}">${t(ready ? 'Sign-in app' : 'Set up sign-in')}</button></small></span><span class="mini-side"><button class="primary" data-account-connect="${escape(p.id)}">${t(n ? 'Add another' : 'Sign in')}</button></span></article>`
}
// Registering the provider's OAuth app happens once; afterwards every account signs in with one click.
function accountAppModal(provider, onSaved) {
  const p = view.team.accountCatalog.find(p => p.id === provider), app = view.team.accountApps?.[provider] ?? {}
  const redirect = view.team.accountRedirect || `${location.origin}/oauth/callback`
  modal(`<h3>${escape(t('{name} sign-in app', { name: p.name }))}</h3>
    <ol class="setup-steps"><li>${t('Create an OAuth client of type “Web application” in the provider console.')} <a href="${escape(p.docs)}" target="_blank" rel="noopener noreferrer">${t('Provider console')}</a></li><li>${t('Add this exact address as an authorized redirect URI:')}<code class="hook">${escape(redirect)}</code></li>${provider === 'google' ? `<li>${t('Enable the APIs you want (Gmail, Calendar, Drive, Docs, Sheets, People, Tasks, Photos Library / Picker), and add yourself as a test user while the app is in testing.')}</li>` : ''}<li>${t('Paste the Client ID and Client Secret here. They stay on this server.')}</li></ol>
    ${app.fromEnvironment ? `<p class="muted">${t('Currently taken from the server environment. Saving here overrides it.')}</p>` : ''}
    <label>Client ID<input id="app-client" autocomplete="off" value="${escape(app.clientId || '')}"></label><label>Client Secret<input id="app-secret" type="password" autocomplete="new-password" placeholder="${t(app.hasClientSecret ? 'Saved; leave empty to keep it' : 'Required by Google and Microsoft')}"></label>
    <div class="form-actions"><button class="ghost" data-close>${t('Cancel')}</button><button class="primary" id="app-save">${t(onSaved ? 'Save and sign in' : 'Save')}</button></div>`)
  $('#app-save').onclick = attempt(async () => {
    await call('account.app.save', { provider, clientId: $('#app-client').value, clientSecret: $('#app-secret').value })
    closeModal(); await refresh()
    if (onSaved) onSaved(); else toast(t('Saved'))
  })
}
// Opens the provider's sign-in page. The window opens before the request so browsers keep it.
async function openSignIn(request) {
  const popup = window.open('about:blank', '_blank')
  try {
    const result = await request()
    if (popup) { popup.opener = null; popup.location.href = result.url }
    else modal(`<h3>${t('Sign in')}</h3><p>${t('Your browser blocked the sign-in window. Open it here, then come back: this page updates by itself.')}</p><div class="form-actions"><button class="ghost" data-close>${t('Close')}</button><a class="button primary" href="${escape(result.url)}" target="_blank" rel="noopener noreferrer">${t('Open sign-in page')}</a></div>`)
    await refresh()
    toast(t('Finish signing in in the new window; the account updates here by itself.'))
  } catch (error) { popup?.close(); throw error }
}
function connectAccount(provider) {
  if (!view.team.accountApps?.[provider]?.configured) return accountAppModal(provider, () => attempt(() => openSignIn(() => call('account.connect', { provider }))) ())
  return openSignIn(() => call('account.connect', { provider }))
}
function accountModal(account) {
  const p = view.team.accountCatalog.find(p => p.id === account.provider)
  const google = account.provider === 'google'
  const rows = google
    ? p.services.map(service => { const access = account.services.find(s => s.id === service.id)?.access ?? 'off'; return `<label class="service-row"><span class="grow"><strong>${escape(t(service.name))}</strong>${service.id === 'photos' ? `<small>${t('Google only lets apps read photos they uploaded or that you pick in the Photos picker.')}</small>` : ''}</span><select data-service="${escape(service.id)}">${['off', 'read', 'write'].map(v => `<option value="${v}" ${v === access ? 'selected' : ''}>${t(ACCESS_TEXT[v])}</option>`).join('')}</select></label>` }).join('')
    : `<label class="check-label"><input id="ac-writes" type="checkbox" ${account.writable ? 'checked' : ''}>${t('Allow account writes')}</label><div class="scope-list">${[...p.scopes.map(scope => [scope, false]), ...p.writeScopes.map(scope => [scope, true])].map(([scope, write]) => `<label class="check-label"><input type="checkbox" data-ac-scope="${escape(scope)}" data-write="${write}" ${account.scopes.includes(scope) ? 'checked' : ''} ${write && !account.writable ? 'disabled' : ''}><span>${escape(scope)}</span></label>`).join('')}</div>`
  modal(`<h3>${escape(t('{name} permissions', { name: account.name }))}</h3>
    <label>${t('Name')}<input id="ac-name" value="${escape(account.name)}"></label><div class="service-list">${rows}</div>
    <div class="form-actions"><button class="ghost" data-close>${t('Cancel')}</button><button class="primary" id="ac-save">${t('Save')}</button></div>`, !google)
  if ($('#ac-writes')) $('#ac-writes').onchange = () => document.querySelectorAll('[data-ac-scope][data-write="true"]').forEach(el => { el.disabled = !$('#ac-writes').checked; if (el.disabled) el.checked = false })
  $('#ac-save').onclick = attempt(async () => {
    const button = $('#ac-save'); button.disabled = true
    try {
      let scopes, writable
      if (google) {
        const levels = Object.fromEntries([...document.querySelectorAll('[data-service]')].map(el => [el.dataset.service, el.value]))
        scopes = [...p.scopes.filter(s => !s.startsWith('https://')), ...p.services.flatMap(s => levels[s.id] === 'off' ? [] : [...s.read, ...(levels[s.id] === 'write' ? s.write : [])])]
        writable = Object.values(levels).includes('write')
      } else {
        scopes = [...document.querySelectorAll('[data-ac-scope]:checked')].map(el => el.dataset.acScope); writable = $('#ac-writes').checked
      }
      const saved = await call('account.save', { id: account.id, provider: account.provider, name: $('#ac-name').value, clientId: account.clientId, writable, enabled: account.enabled, scopes })
      closeModal(); await refresh()
      if (saved.status === 'consent') { if (confirm(t('Sign in again now to grant the added access?'))) await openSignIn(() => call('account.authorize', { id: account.id })) }
      else toast(t('Saved'))
    } finally { button.disabled = false }
  })
}
function deliveriesHtml() {
  const list = (view.team?.deliveries ?? []).filter(d => d.status !== 'delivered' && d.status !== 'cancelled')
  if (!list.length) return ''
  return `<h3>${t('Delivery queue')}</h3><div class="stack">${list.map(d => `<div class="row-card"><div class="grow"><strong>${escape(view.team.channels.find(c => c.id === d.channel)?.name || d.channel)}</strong> <span class="badge">${t(d.status)}</span><p>${d.sent}/${d.total} · ${when(d.updated)}</p>${d.error ? `<p class="error">${escape(d.error)}</p>` : ''}</div><div class="row-actions">${['failed', 'blocked', 'uncertain'].includes(d.status) ? `<button class="ghost" data-delivery-retry="${escape(d.id)}" data-uncertain="${d.status === 'uncertain'}">${t('Retry remaining')}</button>${d.status === 'uncertain' ? `<button class="ghost" data-delivery-skip="${escape(d.id)}">${t('Current part already received')}</button>` : ''}` : ''}</div></div>`).join('')}</div>`
}
function eventEvidence(status = {}) {
  return `${status.received ? `${t('Provider event received')} ${when(status.received)}` : t('No provider delivery verified')}${status.verified ? ` · ${t('Callback verified')} ${when(status.verified)}` : ''}${status.registered ? ` · ${t('Subscription registered')}` : ''}${status.localTest ? ` · ${t('Local test passed')} (${t(status.localMatched ? 'matched' : 'filtered')})` : ''}${status.error ? ` · ${escape(status.error)}` : ''}`
}
async function eventModal(id) {
  const setup = await call('event.setup', { id })
  const routine = (pageData.routines ?? []).find(r => r.id === id)
  const source = setup.source
  const payload = source === 'slack' ? { type: 'event_callback', event_id: `test-${Date.now()}`, event: { type: routine?.when.event || 'message', text: 'dotbot test' } } : { type: routine?.when.event || 'message', ref: 'refs/heads/main', repository: { full_name: 'owner/repo' } }
  for (const [pointer, value] of Object.entries(routine?.when.filters ?? {})) {
    const parts = pointer.slice(1).split('/').map(p => p.replace(/~1/g, '/').replace(/~0/g, '~')); let node = payload
    parts.forEach((key, i) => { if (['__proto__', 'constructor', 'prototype'].includes(key)) return; if (i === parts.length - 1) node[key] = value; else { node[key] ??= {}; node = node[key] } })
  }
  modal(`<h3>${t('Set up and verify events')}</h3><p class="event-evidence">${eventEvidence(setup.status)}</p>${setup.public ? '' : `<p class="error">${t('External providers need a reachable HTTPS public URL.')}</p>`}<label>${t('Public service URL')}<input id="es-origin" value="${escape(setup.public ? setup.publicUrl : '')}" placeholder="https://team.example.com"></label><button class="ghost" id="es-origin-save">${t('Save public URL')}</button><code class="hook">${escape(setup.webhook)}</code><ol>${setup.guide.map(step => `<li>${escape(step)}</li>`).join('')}</ol>
    ${source !== 'webhook' ? `<label>${t('Signing secret key name')}<input id="es-key" value="${escape(routine?.when.signingSecret || '')}" readonly></label><label>${t('Signing secret value')}<input id="es-secret" type="password" autocomplete="new-password"></label><button class="ghost" id="es-secret-save">${t('Save key')}</button>` : ''}
    ${source === 'github' ? `<div class="form-grid"><label>${t('GitHub account')}<select id="es-account">${(view.team.accounts ?? []).filter(a => a.provider === 'github').map(a => `<option value="${escape(a.id)}">${escape(a.name)}</option>`).join('')}</select></label><label>${t('Repository')}<input id="es-repository" value="${escape(setup.status.repository || '')}" placeholder="owner/repo"></label></div><div class="form-actions"><button class="ghost" id="es-register">${t('Register GitHub subscription')}</button><button class="ghost" id="es-provider-test">${t('Request provider test')}</button></div>${setup.status.hook ? `<p>${t('Provider subscription')}: ${t(setup.status.providerActive ? 'Active' : 'Inactive')}${setup.status.providerUrlMatches === false ? ` · ${t('Callback URL differs')}` : ''}</p><div class="form-actions"><button class="ghost" id="es-check">${t('Check subscription')}</button><button class="danger" id="es-remove">${t('Remove provider subscription')}</button></div>` : ''}` : ''}
    <details open><summary>${t('Local signature and filter test')}</summary><textarea id="es-payload" rows="6">${escape(JSON.stringify(payload, null, 2))}</textarea><button class="ghost" id="es-local-test">${t('Test local handler')}</button></details><p id="es-result" role="status"></p><div class="form-actions"><button class="ghost" id="es-refresh">${t('Refresh status')}</button><button class="primary" data-close>${t('Close')}</button></div>`, true)
  const run = action => attempt(async () => { const result = await action(); $('#es-result').textContent = typeof result === 'string' ? result : JSON.stringify(result); await loadPage('routines', false) })
  $('#es-origin-save').onclick = attempt(async () => { await call('event.public-url', { url: $('#es-origin').value.trim() }); await refresh(); await eventModal(id) })
  if ($('#es-secret-save')) $('#es-secret-save').onclick = run(async () => { const name = $('#es-key').value; if (!name) throw new Error(t('Set the signing key name in the routine first.')); await call('secret.set', { name, value: $('#es-secret').value }); $('#es-secret').value = ''; return t('Saved') })
  if ($('#es-register')) $('#es-register').onclick = run(() => call('event.register', { id, account: $('#es-account').value, repository: $('#es-repository').value.trim() }))
  if ($('#es-provider-test')) $('#es-provider-test').onclick = run(async () => { await call('event.provider-test', { id }); return t('Test requested. Wait for a real provider arrival, then refresh status.') })
  if ($('#es-check')) $('#es-check').onclick = attempt(async () => { await call('event.subscription', { id }); await eventModal(id) })
  if ($('#es-remove')) $('#es-remove').onclick = attempt(async () => { if (confirm(t('Remove this webhook subscription from the provider?'))) { await call('event.subscription', { id, delete: true }); await eventModal(id) } })
  $('#es-local-test').onclick = run(async () => { const result = await call('event.test', { id, payload: JSON.parse($('#es-payload').value) }); return t(result.matched ? 'Local test passed; filters matched. No model work was dispatched.' : 'Local test passed; payload was filtered. No model work was dispatched.') })
  $('#es-refresh').onclick = attempt(() => eventModal(id))
}

let connectionEditing = null
const pairsText = map => Object.entries(map ?? {}).map(([k, v]) => `${k}=${v}`).join('\n')
let connTab = null
const CONN_TABS = [['accounts', 'Connected accounts'], ['add', 'Add account'], ['keys', 'Keys']]
// Connections that refer to a key as ${NAME} in their environment or headers.
const keyUsers = name => (view.team?.connections ?? []).filter(c => Object.values({ ...c.env, ...c.headers }).some(v => String(v).includes('${' + name + '}'))).map(c => mcpEntryFor(c)?.name ?? c.name)
function keyMiniCard(name) {
  const users = keyUsers(name)
  return `<article class="mini-card"><span class="mini-icon">${icon('key')}</span><span class="mini-main"><strong title="${escape(name)}"><code>${escape(name)}</code></strong><small>${escape(users.length ? t('Used by {names}', { names: users.join('、') }) : `$DOTBOT_SECRET_${name}`)}</small></span><span class="mini-side"><button class="ghost" data-secret-delete="${escape(name)}">${t('Delete')}</button></span></article>`
}
function keyModal() {
  modal(`<h3>${t('Save key')}</h3><label>${t('Name')}<input id="sc-name" placeholder="GITHUB_TOKEN" spellcheck="false" autocomplete="off"><p class="muted field-hint">${t('English letters, digits and _')}</p></label><label>${t('Value')}<input id="sc-value" type="password" autocomplete="new-password"></label>
    <div class="form-actions"><button class="ghost" data-close>${t('Cancel')}</button><button class="primary" id="sc-save">${t('Save key')}</button></div>`)
  $('#sc-name').focus()
}
function connectionsHtml() {
  const accounts = view.team?.accounts ?? [], secrets = view.team?.secrets ?? [], catalog = view.team?.accountCatalog ?? []
  connTab ??= accounts.length ? 'accounts' : 'add'
  const counts = { accounts: accounts.length, keys: secrets.length }
  const body = {
    accounts: () => listHtml('accounts', { items: () => view.team?.accounts ?? [], text: a => `${a.name} ${a.identity ?? ''} ${a.provider}`, card: accountMiniCard, size: 8, placeholder: t('Search accounts'), empty: query => query ? t('No matching accounts.') : t('No accounts yet.') }, `<button class="primary list-add" data-conn-tab="add">＋ ${t('Add an account')}</button>`),
    add: () => listHtml('account-store', { items: () => view.team?.accountCatalog ?? [], text: p => `${p.name} ${ACCOUNT_USE[p.id] ? ACCOUNT_USE[p.id].join(' ') : ''}`, card: accountStoreCard, size: 12, placeholder: t('Search services'), empty: () => t('No matching services.') }),
    keys: () => listHtml('keys', { items: () => view.team?.secrets ?? [], text: n => n, card: keyMiniCard, size: 12, placeholder: t('Search keys'), empty: query => query ? t('No matching keys.') : t('No keys saved.') }, `<button class="primary list-add" data-key-new>＋ ${t('Save key')}</button>`),
  }[connTab]()
  return `<div class="page conn-page plugins-page"><div class="plugin-tabs" role="tablist">${CONN_TABS.map(([key, name]) => `<button role="tab" class="${connTab === key ? 'on' : ''}" aria-selected="${connTab === key}" data-conn-tab="${key}">${t(name)}${key in counts ? ` <span class="conn-count">${counts[key]}</span>` : ''}</button>`).join('')}</div>${body}</div>`
}

// Messaging platforms: what each needs, as [setting, label, input type, placeholder].
const CHANNEL_KINDS = {
  telegram: { name: 'Telegram', help: 'Make a bot with @BotFather and paste its token. In a group it answers when called by @name or answered.', fields: [['token', 'Bot token', 'password', '123456:ABC…'], ['apiBase', 'API address (optional, for a proxy)', 'text', 'https://api.telegram.org']] },
  feishu: { name: '飞书 / Lark', help: 'Make a custom app in the Feishu open platform, turn on its bot, choose "receive events through a long connection", subscribe to im.message.receive_v1 and give it permission to send messages.', fields: [['appId', 'App ID', 'text', 'cli_…'], ['appSecret', 'App Secret', 'password', ''], ['domain', 'Region', 'select:feishu=Feishu (China),lark=Lark (international)', '']] },
  dingtalk: { name: '钉钉', help: 'Make an app with a robot in the DingTalk developer console and set its message receiving mode to Stream.', fields: [['clientId', 'Client ID (AppKey)', 'text', 'ding…'], ['clientSecret', 'Client Secret (AppSecret)', 'password', '']] },
  qq: { name: 'QQ', help: 'Make a bot on the QQ open platform (q.qq.com) and turn on direct, group and channel message events. Replies are passive: QQ takes them for a while after each message.', fields: [['appId', 'AppID', 'text', '1020…'], ['appSecret', 'AppSecret', 'password', ''], ['sandbox', 'Sandbox', 'checkbox', '']] },
  wechat: { name: '微信', help: 'Save it, then press Sign in and scan the code with WeChat (WeChat\'s ClawBot / iLink bot). The person who scans is allowed.', fields: [] },
}
const STATE_TEXT = { off: 'Off', connecting: 'Connecting…', online: 'Online', login: 'Needs sign-in', error: 'Not connected' }
let channelEditing = null

function channelFieldsHtml(kind, settings = {}) {
  return (CHANNEL_KINDS[kind]?.fields ?? []).map(([key, label, type, placeholder]) => {
    const value = settings[key] ?? ''
    if (type === 'checkbox') return `<label class="check"><input id="ch-f-${key}" type="checkbox" ${value ? 'checked' : ''}> ${t(label)}</label>`
    if (type.startsWith('select:')) return `<label>${t(label)}<select id="ch-f-${key}">${type.slice(7).split(',').map(o => o.split('=')).map(([v, n]) => `<option value="${v}" ${v === value ? 'selected' : ''}>${t(n)}</option>`).join('')}</select></label>`
    return `<label>${t(label)}<input id="ch-f-${key}" type="${type}" ${['token', 'appId', 'appSecret', 'clientId', 'clientSecret'].includes(key) ? 'required' : ''} autocomplete="off" placeholder="${escape(placeholder)}" value="${escape(value)}"></label>`
  }).join('')
}

// Platform marks (Simple Icons / Ant Design / allogo; trademarks of their owners, used only to name the platform) and brand colors.
const CHANNEL_BADGES = {
  telegram: ['<svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><path d="M11.944 0A12 12 0 0 0 0 12a12 12 0 0 0 12 12 12 12 0 0 0 12-12A12 12 0 0 0 12 0a12 12 0 0 0-.056 0zm4.962 7.224c.1-.002.321.023.465.14a.506.506 0 0 1 .171.325c.016.093.036.306.02.472-.18 1.898-.962 6.502-1.36 8.627-.168.9-.499 1.201-.82 1.23-.696.065-1.225-.46-1.9-.902-1.056-.693-1.653-1.124-2.678-1.8-1.185-.78-.417-1.21.258-1.91.177-.184 3.247-2.977 3.307-3.23.007-.032.014-.15-.056-.212s-.174-.041-.249-.024c-.106.024-1.793 1.14-5.061 3.345-.48.33-.913.49-1.302.48-.428-.008-1.252-.241-1.865-.44-.752-.245-1.349-.374-1.297-.789.027-.216.325-.437.893-.663 3.498-1.524 5.83-2.529 6.998-3.014 3.332-1.386 4.025-1.627 4.476-1.635z"/></svg>', '#26a5e4'],
  feishu: ['<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12.9238 12.8029C12.9427 12.784 12.9616 12.7682 12.9806 12.7493C13.0184 12.7146 13.0563 12.6767 13.091 12.6389L13.1667 12.5631L13.397 12.336L14.7315 11.0173L15.0659 10.686C15.129 10.6229 15.1952 10.563 15.2615 10.5031C15.3845 10.3926 15.5076 10.2854 15.6369 10.1813C15.7536 10.0866 15.8767 9.99514 15.9997 9.9068C16.1732 9.78376 16.3499 9.67019 16.5329 9.55977C16.7127 9.45251 16.8957 9.35471 17.085 9.26322C17.2616 9.17804 17.4415 9.09917 17.6276 9.02661C17.7317 8.9856 17.8326 8.94774 17.9399 8.91304C17.9935 8.89411 18.044 8.87834 18.0977 8.86256C17.6276 7.00439 16.7632 5.3008 15.5991 3.84959C15.3719 3.56566 15.0249 3.40161 14.6589 3.40161H5.0084C4.83489 3.40161 4.76233 3.6256 4.90114 3.72656C8.18528 6.13997 10.9236 9.24114 12.9017 12.825C12.908 12.8187 12.9175 12.8124 12.9238 12.8029Z" fill="#00D6B9"/><path d="M9.09696 21.2986C14.0815 21.2986 18.4225 18.5476 20.6877 14.4843C20.7666 14.3423 20.8454 14.1972 20.918 14.052C20.8044 14.2729 20.6751 14.4811 20.5394 14.6767C20.4889 14.7461 20.4385 14.8155 20.388 14.8818C20.3217 14.9669 20.2555 15.049 20.1861 15.1278C20.1324 15.1909 20.0757 15.2509 20.0189 15.3108C19.9021 15.4307 19.7823 15.5474 19.6561 15.6547C19.5867 15.7146 19.5141 15.7714 19.4415 15.8282C19.3564 15.8944 19.268 15.9575 19.1797 16.0143C19.1229 16.0522 19.0661 16.09 19.0093 16.1247C18.9494 16.1626 18.8895 16.1973 18.8264 16.232C18.7002 16.3014 18.574 16.3645 18.4446 16.4245C18.3311 16.4749 18.2175 16.5223 18.1008 16.5633C17.9746 16.6106 17.8452 16.6516 17.7159 16.6863C17.5234 16.7399 17.3247 16.7809 17.1259 16.8125C16.9808 16.8346 16.8357 16.8504 16.6874 16.863C16.5328 16.8724 16.3751 16.8787 16.2173 16.8756C16.0438 16.8724 15.8703 16.863 15.6936 16.844C15.5643 16.8314 15.435 16.8125 15.3056 16.7873C15.192 16.7683 15.0785 16.7431 14.9649 16.7178C14.9049 16.7021 14.845 16.6895 14.7851 16.6737C14.6179 16.6295 14.4538 16.5822 14.2898 16.5349C14.2077 16.5096 14.1257 16.4875 14.0437 16.4623C13.9206 16.4245 13.7976 16.3897 13.6777 16.3519C13.5768 16.3203 13.479 16.2888 13.378 16.2572C13.2834 16.2257 13.1887 16.1942 13.0941 16.1626C13.031 16.1405 12.9647 16.1184 12.9016 16.0964C12.8228 16.0711 12.7471 16.0427 12.6682 16.0143C12.6114 15.9954 12.5578 15.9765 12.501 15.9544C12.3906 15.9134 12.2802 15.8755 12.1729 15.8345C12.1098 15.8093 12.0467 15.7872 11.9836 15.7619C11.8984 15.7304 11.8132 15.6957 11.7312 15.6641C11.6429 15.6294 11.5514 15.5947 11.4631 15.5569C11.4063 15.5348 11.3463 15.5096 11.2895 15.4875C11.217 15.4591 11.1476 15.4275 11.075 15.3991C11.0214 15.3771 10.9646 15.3518 10.911 15.3297C10.8542 15.3045 10.7974 15.2793 10.7406 15.254C10.6901 15.2319 10.6428 15.2099 10.5923 15.1878C10.5482 15.1688 10.5008 15.1468 10.4567 15.1278C10.4094 15.1057 10.3652 15.0868 10.3179 15.0647C10.2705 15.0427 10.2232 15.0206 10.1759 14.9985C10.116 14.9701 10.056 14.9417 9.99608 14.9165C9.93299 14.8881 9.87304 14.8565 9.80995 14.8281C9.7437 14.7966 9.67745 14.765 9.6112 14.7303C9.55441 14.7019 9.49762 14.6735 9.44084 14.6483C6.45324 13.1592 3.80321 11.1717 1.54438 8.76145C1.43081 8.64157 1.23206 8.72044 1.23206 8.88449L1.23836 18.0933C1.23836 18.494 1.43712 18.8726 1.77153 19.0934C3.86631 20.4878 6.38699 21.2986 9.09696 21.2986Z" fill="#3370FF"/><path d="M23.7322 9.29488C22.7226 8.79642 21.5838 8.5188 20.3818 8.5188C19.6688 8.5188 18.9747 8.6166 18.3217 8.80273C18.246 8.82481 18.1703 8.8469 18.0977 8.86898C18.0441 8.88476 17.9905 8.90368 17.94 8.91946C17.8359 8.95416 17.7318 8.99202 17.6276 9.03303C17.4447 9.10559 17.2617 9.18446 17.085 9.26964C16.8957 9.36113 16.7128 9.45893 16.5329 9.56619C16.35 9.67345 16.1701 9.79018 15.9998 9.91322C15.8767 10.0016 15.7569 10.093 15.637 10.1877C15.5076 10.2918 15.3846 10.3991 15.2616 10.5095C15.1953 10.5694 15.1322 10.6325 15.066 10.6925L14.7315 11.0206L13.3939 12.3424L13.1636 12.5696L13.0879 12.6453C13.05 12.6831 13.0122 12.7178 12.9775 12.7557C12.9586 12.7746 12.9396 12.7904 12.9207 12.8093C12.8923 12.8377 12.8639 12.863 12.8355 12.8882C12.804 12.9166 12.7724 12.9481 12.7409 12.9765C11.9143 13.7368 10.9931 14.3899 9.99304 14.923C10.053 14.9514 10.1129 14.9798 10.1729 15.0051C10.2202 15.0271 10.2675 15.0492 10.3148 15.0713C10.359 15.0934 10.4063 15.1123 10.4536 15.1344C10.4978 15.1533 10.5451 15.1754 10.5893 15.1943C10.6398 15.2164 10.6871 15.2385 10.7376 15.2606C10.7944 15.2858 10.8511 15.3111 10.9079 15.3363C10.9616 15.3584 11.0184 15.3836 11.072 15.4057C11.1445 15.4373 11.2139 15.4657 11.2865 15.4941C11.3433 15.5193 11.4032 15.5414 11.46 15.5635C11.5484 15.5982 11.6367 15.636 11.7282 15.6707C11.8134 15.7023 11.8954 15.737 11.9806 15.7685C12.0437 15.7938 12.1068 15.8158 12.1699 15.8411C12.2803 15.8821 12.3875 15.9231 12.498 15.961C12.5547 15.9799 12.6084 16.002 12.6652 16.0209C12.744 16.0493 12.8197 16.0745 12.8986 16.1029C12.9617 16.125 13.028 16.1471 13.0911 16.1692C13.1857 16.2007 13.2803 16.2323 13.375 16.2638C13.4728 16.2954 13.5737 16.3269 13.6747 16.3585C13.7977 16.3963 13.9176 16.4342 14.0406 16.4689C14.1227 16.4941 14.2047 16.5162 14.2867 16.5414C14.4508 16.5888 14.618 16.6361 14.782 16.6803C14.842 16.696 14.9019 16.7118 14.9618 16.7244C15.0754 16.7528 15.189 16.7749 15.3026 16.7938C15.4319 16.8159 15.5613 16.8348 15.6906 16.8506C15.8673 16.8695 16.0408 16.8822 16.2143 16.8822C16.372 16.8853 16.5298 16.879 16.6844 16.8695C16.8326 16.8601 16.9778 16.8412 17.1229 16.8191C17.3248 16.7875 17.5204 16.7465 17.7128 16.6929C17.8422 16.6582 17.9715 16.6172 18.0977 16.5698C18.2144 16.5257 18.328 16.4815 18.4416 16.4279C18.5709 16.3679 18.7003 16.3048 18.8233 16.2354C18.8833 16.2007 18.9464 16.166 19.0063 16.1282C19.0631 16.0935 19.1199 16.0556 19.1767 16.0178C19.265 15.9578 19.3533 15.8947 19.4385 15.8316C19.5111 15.7748 19.5836 15.718 19.653 15.6581C19.7792 15.5508 19.8991 15.4341 20.0158 15.3142C20.0726 15.2543 20.1294 15.1943 20.183 15.1313C20.2524 15.0524 20.3187 14.9704 20.3849 14.8852C20.4354 14.8189 20.4859 14.7495 20.5364 14.6801C20.672 14.4845 20.7982 14.2763 20.9118 14.0586L21.0411 13.7999L22.2084 11.4748L22.2053 11.4812C22.5807 10.6578 23.1012 9.91953 23.7322 9.29488Z" fill="#133C9A"/></svg>', '#3370ff'],
  dingtalk: ['<svg viewBox="0 0 1024 1024" fill="currentColor" aria-hidden="true"><path d="M512 64C264.6 64 64 264.6 64 512s200.6 448 448 448s448-200.6 448-448S759.4 64 512 64m227 385.3c-1 4.2-3.5 10.4-7 17.8h.1l-.4.7c-20.3 43.1-73.1 127.7-73.1 127.7s-.1-.2-.3-.5l-15.5 26.8h74.5L575.1 810l32.3-128h-58.6l20.4-84.7c-16.5 3.9-35.9 9.4-59 16.8c0 0-31.2 18.2-89.9-35c0 0-39.6-34.7-16.6-43.4c9.8-3.7 47.4-8.4 77-12.3c40-5.4 64.6-8.2 64.6-8.2S422 517 392.7 512.5c-29.3-4.6-66.4-53.1-74.3-95.8c0 0-12.2-23.4 26.3-12.3s197.9 43.2 197.9 43.2s-207.4-63.3-221.2-78.7s-40.6-84.2-37.1-126.5c0 0 1.5-10.5 12.4-7.7c0 0 153.3 69.7 258.1 107.9c104.8 37.9 195.9 57.3 184.2 106.7"/></svg>', '#247af5'],
  qq: ['<svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><path d="M21.395 15.035a40 40 0 0 0-.803-2.264l-1.079-2.695c.001-.032.014-.562.014-.836C19.526 4.632 17.351 0 12 0S4.474 4.632 4.474 9.241c0 .274.013.804.014.836l-1.08 2.695a39 39 0 0 0-.802 2.264c-1.021 3.283-.69 4.643-.438 4.673.54.065 2.103-2.472 2.103-2.472 0 1.469.756 3.387 2.394 4.771-.612.188-1.363.479-1.845.835-.434.32-.379.646-.301.778.343.578 5.883.369 7.482.189 1.6.18 7.14.389 7.483-.189.078-.132.132-.458-.301-.778-.483-.356-1.233-.646-1.846-.836 1.637-1.384 2.393-3.302 2.393-4.771 0 0 1.563 2.537 2.103 2.472.251-.03.581-1.39-.438-4.673"/></svg>', '#1ebafc'],
  wechat: ['<svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><path d="M8.691 2.188C3.891 2.188 0 5.476 0 9.53c0 2.212 1.17 4.203 3.002 5.55a.59.59 0 0 1 .213.665l-.39 1.48c-.019.07-.048.141-.048.213 0 .163.13.295.29.295a.326.326 0 0 0 .167-.054l1.903-1.114a.864.864 0 0 1 .717-.098 10.16 10.16 0 0 0 2.837.403c.276 0 .543-.027.811-.05-.857-2.578.157-4.972 1.932-6.446 1.703-1.415 3.882-1.98 5.853-1.838-.576-3.583-4.196-6.348-8.596-6.348zM5.785 5.991c.642 0 1.162.529 1.162 1.18a1.17 1.17 0 0 1-1.162 1.178A1.17 1.17 0 0 1 4.623 7.17c0-.651.52-1.18 1.162-1.18zm5.813 0c.642 0 1.162.529 1.162 1.18a1.17 1.17 0 0 1-1.162 1.178 1.17 1.17 0 0 1-1.162-1.178c0-.651.52-1.18 1.162-1.18zm5.34 2.867c-1.797-.052-3.746.512-5.28 1.786-1.72 1.428-2.687 3.72-1.78 6.22.942 2.453 3.666 4.229 6.884 4.229.826 0 1.622-.12 2.361-.336a.722.722 0 0 1 .598.082l1.584.926a.272.272 0 0 0 .14.047c.134 0 .24-.111.24-.247 0-.06-.023-.12-.038-.177l-.327-1.233a.582.582 0 0 1-.023-.156.49.49 0 0 1 .201-.398C23.024 18.48 24 16.82 24 14.98c0-3.21-2.931-5.837-6.656-6.088V8.89c-.135-.01-.27-.027-.407-.03zm-2.53 3.274c.535 0 .969.44.969.982a.976.976 0 0 1-.969.983.976.976 0 0 1-.969-.983c0-.542.434-.982.97-.982zm4.844 0c.535 0 .969.44.969.982a.976.976 0 0 1-.969.983.976.976 0 0 1-.969-.983c0-.542.434-.982.969-.982z"/></svg>', '#07c160'],
}
const platformBadge = kind => { const [mark, color] = CHANNEL_BADGES[kind] ?? ['?', '#888']; return `<span class="platform-badge" style="--platform-color:${color}">${mark}</span>` }
let channelKind = 'telegram'
let channelDrafts = {}
// Today shows only the time; earlier days add the date (and the year when it differs).
const channelTime = value => {
  const date = value ? new Date(value) : null
  if (!date || Number.isNaN(date.getTime())) return t('Never')
  const now = new Date(), clock = date.toLocaleTimeString(locale(), { hour: '2-digit', minute: '2-digit' })
  const yesterday = new Date(now.getFullYear(), now.getMonth(), now.getDate() - 1)
  if (date.toDateString() === now.toDateString()) return t('Today {time}', { time: clock })
  if (date.toDateString() === yesterday.toDateString()) return t('Yesterday {time}', { time: clock })
  return date.toLocaleString(locale(), { year: date.getFullYear() === now.getFullYear() ? undefined : 'numeric', month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' })
}

function channelCardsHtml(channels = view.team?.channels ?? []) {
  const targets = [...bots().map(b => [b.conversation, b.name]), ...rooms().map(r => [r.conversation, `# ${r.name}`])]
  return channels.map(c => {
    const status = c.status ?? {}, state = c.enabled ? status.state || 'connecting' : 'off'
    const pending = (status.pending ?? []).map(p => `<div class="pending"><span><strong>${escape(p.name)}</strong><code>${escape(p.id)}</code></span><button class="ghost" data-channel-allow="${escape(c.id)}" data-sender="${escape(p.id)}">${t('Allow')}</button></div>`).join('')
    const allow = (c.allow ?? []).map(sender => `<span class="sender-chip"><code>${escape(sender === '*' ? t('anyone') : sender)}</code><button data-channel-revoke="${escape(c.id)}" data-sender="${escape(sender)}" title="${t('Revoke access')}">×</button></span>`).join('') || `<span class="muted">${t('nobody yet')}</span>`
    return `<article class="channel-card" data-channel-card="${escape(c.id)}"><div class="channel-card-head">${platformBadge(c.kind)}<div class="grow"><strong>${escape(c.name)}</strong><p>${escape(CHANNEL_KINDS[c.kind]?.name ?? c.kind)}<span>→</span>${escape(targets.find(([key]) => key === c.target)?.[1] ?? t('Target unavailable'))}</p></div><span class="channel-state state-${escape(state)}"><i></i>${t(STATE_TEXT[state] ?? state)}</span></div>
      ${status.detail ? `<p class="channel-detail">${escape(t(status.detail))}</p>` : ''}
      ${status.lastError ? `<div class="channel-error" role="status"><strong>${t('Reply delivery failed')}</strong><p>${escape(status.lastError)}</p><span>${t('Check the platform permissions and reply window, then try again.')}</span></div>` : ''}
      ${status.qr && c.enabled ? `<div class="qr">${status.qr}<p>${t('Scan with WeChat, then confirm on the phone.')}</p></div>` : ''}
      <div class="channel-activity"><span>${t('Last received')}<b title="${escape(status.lastReceived ? new Date(status.lastReceived).toLocaleString(locale()) : '')}">${escape(channelTime(status.lastReceived))}</b></span><span>${t('Last sent')}<b title="${escape(status.lastSent ? new Date(status.lastSent).toLocaleString(locale()) : '')}">${escape(channelTime(status.lastSent))}</b></span></div>
      ${pending ? `<div class="channel-pending"><strong>${t('Waiting for access')}</strong>${pending}</div>` : ''}
      <details class="channel-access" data-keep="${escape(c.id)}" data-channel-access="${escape(c.id)}"><summary>${t('Allowed senders')}<span>${c.allow?.length ?? 0}</span></summary><div class="sender-chips">${allow}</div></details>
      <div class="channel-actions">${c.kind === 'wechat' && (!c.signedIn || ['login', 'error'].includes(state)) ? `<button class="${c.signedIn ? 'ghost' : 'primary'}" data-channel-login="${escape(c.id)}" ${c.enabled ? '' : 'disabled'}>${t(status.qr ? 'New QR code' : c.signedIn ? 'Sign in again' : 'Sign in')}</button>` : ''}<button class="ghost" data-channel-edit="${escape(c.id)}">${t('Edit')}</button><button class="ghost" data-channel-diagnostics="${escape(c.id)}">延迟诊断</button><button class="ghost" data-channel-reconnect="${escape(c.id)}" ${c.enabled ? '' : 'disabled'}>${t('Reconnect')}</button><button class="ghost" data-channel-toggle="${escape(c.id)}">${c.enabled ? t('Pause') : t('Resume')}</button><button class="channel-remove" data-channel-delete="${escape(c.id)}">${t('Remove')}</button></div></article>`
  }).join('') || `<div class="channel-empty">${icon('channels')}<strong>${t('No platforms connected yet.')}</strong><p>${t('Choose a platform below and connect it to a Bot or group chat.')}</p></div>`
}

const channelTargets = () => [...bots().map(b => [b.conversation, b.name]), ...rooms().map(r => [r.conversation, `# ${r.name}`])]
function channelMiniCard(c) {
  const status = c.status ?? {}, state = c.enabled ? status.state || 'connecting' : 'off'
  const tone = state === 'online' ? 'on' : state === 'error' ? 'err' : state === 'login' ? 'warn' : ''
  const target = channelTargets().find(([key]) => key === c.target)?.[1] ?? t('Target unavailable')
  const waiting = (status.pending ?? []).length
  return `<button class="mini-card ${c.enabled ? '' : 'is-off'}" data-detail="channel:${escape(c.id)}"><span class="mini-icon">${platformBadge(c.kind)}</span><span class="mini-main"><strong>${escape(c.name)}</strong><small>${escape(CHANNEL_KINDS[c.kind]?.name ?? c.kind)} → ${escape(target)}${waiting ? ` · ${t('Waiting for access')} ${waiting}` : ''}</small></span><span class="mini-status ${tone}">${t(STATE_TEXT[state] ?? state)}</span></button>`
}
DETAILS.channel = id => {
  const c = (view.team?.channels ?? []).find(x => x.id === id)
  return c ? channelCardsHtml([c]) : ''
}
// Status changes redraw only the list, the deliveries and an open detail, so a search keeps its focus.
function refreshChannelCards() {
  if ($('#delivery-list')) $('#delivery-list').innerHTML = deliveriesHtml()
  updateList('channels')
  refreshDetail()
}

function channelsHtml() {
  const channels = view.team?.channels ?? []
  return `<div class="page channels-page"><div id="delivery-list">${deliveriesHtml()}</div>
    <div class="settings-section-title"><h3>${t('Connected platforms')}</h3><span>${channels.length}</span></div>
    ${listHtml('channels', { items: () => view.team?.channels ?? [], text: c => `${c.name} ${CHANNEL_KINDS[c.kind]?.name ?? c.kind}`, card: channelMiniCard, size: 8, placeholder: t('Search platforms'), empty: query => query ? t('No matching platforms.') : `${t('No platforms connected yet.')} ${t('Choose a platform below and connect it to a Bot or group chat.')}` })}
    <div class="settings-section-title channel-add-title"><h3 id="ch-heading">${t('Connect a platform')}</h3></div>
    <div class="mini-grid">${Object.entries(CHANNEL_KINDS).map(([key, value]) => `<button class="mini-card" data-channel-new="${key}"><span class="mini-icon">${platformBadge(key)}</span><span class="mini-main"><strong>${escape(value.name)}</strong><small class="wrap">${escape(t(value.help))}</small></span><span class="mini-status">＋ ${t('Add')}</span></button>`).join('')}</div></div>`
}
// The connect/edit form opens in a window; its #ch-* fields are read when saving.
function channelFormHtml() {
  const channels = view.team?.channels ?? [], targets = channelTargets()
  const editing = channels.find(c => c.id === channelEditing)
  const kind = editing?.kind ?? channelKind
  return `<h3 id="ch-form-heading">${editing ? t('Edit {name}', { name: escape(editing.name) }) : t('Connect a platform')}</h3>
    <div id="ch-form"><div class="form-grid">
      <label>${t('Platform')}<select id="ch-kind" ${editing ? 'disabled' : ''}>${Object.entries(CHANNEL_KINDS).map(([key, value]) => `<option value="${key}" ${key === kind ? 'selected' : ''}>${escape(value.name)}</option>`).join('')}</select></label>
      <label>${t('Messages go to')}<select id="ch-target">${targets.map(([key, name]) => `<option value="${escape(key)}" ${key === editing?.target ? 'selected' : ''}>${escape(name)}</option>`).join('')}</select></label>
      <label class="full">${t('Name')}<input id="ch-name" maxlength="80" placeholder="${escape(CHANNEL_KINDS[kind].name)}" value="${escape(editing?.name ?? '')}"></label>
      <div class="channel-guide full"><span>${icon('connections')}</span><p id="ch-help">${t(CHANNEL_KINDS[kind].help)}</p></div>
      <div class="form-grid full" id="ch-fields">${channelFieldsHtml(kind, editing?.settings ?? channelDrafts[kind])}</div>
      <label class="full">${t('Allowed senders, one per line (* for anyone)')}<textarea id="ch-allow" rows="2" placeholder="${t('Leave empty and approve senders as they arrive.')}">${escape((editing?.allow ?? []).join('\n'))}</textarea></label>
    </div><div id="ch-error" class="channel-error" role="alert" hidden></div>
    <div class="form-actions"><button class="ghost" id="ch-cancel">${t('Cancel')}</button><button class="primary" id="ch-save">${editing ? t('Save') : t('Connect')}</button></div></div>`
}
function openChannelForm() { modal(channelFormHtml(), true); bindChannelForm() }

function bindChannelForm() {
  const kind = $('#ch-kind')
  if (!kind) return
  let previous = kind.value
  kind.onchange = () => {
    channelDrafts[previous] = Object.fromEntries((CHANNEL_KINDS[previous]?.fields ?? []).map(([key, , type]) => [key, type === 'checkbox' ? ($(`#ch-f-${key}`).checked ? '1' : '') : $(`#ch-f-${key}`).value]))
    previous = channelKind = kind.value
    $('#ch-fields').innerHTML = channelFieldsHtml(kind.value, channelDrafts[kind.value])
    $('#ch-help').textContent = t(CHANNEL_KINDS[kind.value].help)
    $('#ch-name').placeholder = CHANNEL_KINDS[kind.value].name
    $('#ch-error').hidden = true
  }
}

// ---------------------------------------------------------------------------
// The panel

let panelTicket = 0
let lastPanel = null
function openPanel(kind) {
  ++panelTicket
  renderedMarkup.delete($('#panel-body'))
  const desktopBrowser = kind === 'browser' && view.team?.desktop
  if (desktopBrowser) kind = 'desktop'
  view.panel = kind
  lastPanel = kind
  $('#panel').hidden = false
  const bot = currentBot()
  const room = currentRoom()
  $('#panel-body').setAttribute('aria-busy', String(kind === 'memory'))
  stopLive()
  const desktop = kind === 'desktop'
  $('#desktop-host').hidden = !desktop
  $('#desktop-toolbar').hidden = !desktop
  $('#panel').classList.toggle('desktop-mode', Boolean(desktop))
  if (!desktop) giveBackDesktop()
  if (kind === 'details') bot ? botDetails(bot) : room ? roomDetails(room) : closePanel()
  else if (kind === 'memory' && (bot || room)) memoryPanel(bot, room)
  else if (kind === 'browser' && bot) browserPanel(bot)
  else if (kind === 'desktop' && bot) desktopPanel(bot)
  else if (kind === 'local' && bot) localPanel()
  else if (kind === 'activity' && bot) {
    $('#panel-title').textContent = t('Team activity')
    $('#panel-body').innerHTML = '<div id="activity-history"></div>'
  }
  else closePanel()
  renderPanelTabs()
  renderHead()
  if (desktopBrowser) $('#d-url')?.focus()
}
// Details, memory and team activity are views of the same conversation: switch in place.
function renderPanelTabs() {
  const tabs = [['details', 'Details'], ['memory', 'Memory'], ...(currentBot() ? [['activity', 'Team activity']] : [])]
  const shown = Boolean((currentBot() || currentRoom()) && tabs.some(([k]) => k === view.panel))
  $('#panel-tabs').hidden = !shown
  $('#panel-title').hidden = shown
  stableHtml($('#panel-tabs'), shown ? tabs.map(([k, label]) => `<button type="button" data-panel-tab="${k}" class="${view.panel === k ? 'on' : ''}" aria-pressed="${view.panel === k}">${t(label)}</button>`).join('') : '')
}
function closePanel(dismiss = true) { ++panelTicket; $('#panel-body').setAttribute('aria-busy', 'false'); stopLive(); giveBackDesktop(); view.panel = null; $('#panel').hidden = true; $('#panel').classList.remove('wide', 'desktop-mode'); $('#desktop-toolbar').hidden = true; renderHead() }

function localStatusHtml(status = view.team?.localComputer ?? {}) {
  const owner = status.owner ? botById(status.owner.split(':').at(-1))?.name || status.owner : ''
  return `<div class="local-state ${!status.enabled ? 'is-off' : status.paused ? 'is-paused' : 'is-ready'}"><span class="routine-dot ${status.enabled && !status.paused ? 'enabled' : ''}"></span><strong>${t(!status.enabled ? 'Off' : status.paused ? 'Paused' : 'Ready')}</strong><span>${escape(status.platform || '—')} · ${escape(status.shell || 'bash')}</span>${owner ? `<span>${escape(t('Reserved by {name}', { name: owner }))}</span>` : ''}</div><p class="local-capability muted ${status.commands ? 'available' : 'unavailable'}">${t(status.commands ? 'Commands and file collaboration are available.' : 'Bash is unavailable on this server.')}</p>`
}
function renderLocalStatus() {
  const status = view.team?.localComputer
  if (!status) return
  for (const id of ['local-state', 's-local-state']) if ($(`#${id}`)) $(`#${id}`).innerHTML = localStatusHtml(status)
  if ($('#s-local')) $('#s-local').checked = status.enabled
  if ($('#s-local-body')) $('#s-local-body').hidden = !status.enabled
  for (const id of ['local-pause', 's-local-pause']) if ($(`#${id}`)) {
    $(`#${id}`).textContent = t(status.paused ? 'Resume local commands' : 'Pause local commands')
    $(`#${id}`).disabled = !status.enabled
    $(`#${id}`).onclick = attempt(async () => {
      view.team.localComputer = await call('local.pause', { paused: !view.team.localComputer.paused })
      renderLocalStatus()
    })
  }
}
function localPanel() {
  $('#panel-title').textContent = t('Local terminal')
  $('#panel-body').innerHTML = `<div class="local-card"><div class="local-terminal-symbol">${icon('terminal')}</div><div id="local-state"></div><label>${t('Default working folder')}</label><code class="local-folder">${escape(currentBot()?.workDir || view.team.workspace)}</code><div class="local-actions"><button class="primary" id="local-pause"></button></div></div>`
  renderLocalStatus()
}


// The Bot's independent screen on the shared Linux computer, live. The frame sits outside the panel body, so it stays
// connected while other panels open or the panel closes, and it reconnects when the desktop
// comes back after a restart. It shows as a thumbnail while the Bot works it; taking control
// opens the same frame in a popup. The frame is always DESK_W × DESK_H and only scaled, so
// the remote screen keeps its size and moving between the two never reloads it. The server
// holds the Bot's desktop steps while the user has it, renewed every 20 s.
const desk = { bot: null, user: false, renew: null, phase: 'idle', ticket: 0, observer: null, launching: false }
const DESK_W = 1280, DESK_H = 900
function fitDesktop() {
  const view = $('#d-view'), frame = view?.querySelector('.desktop-frame'), stage = view?.querySelector('.desktop-stage')
  if (!frame || !stage) return
  const scale = view.classList.contains('popup')
    ? Math.min((innerWidth - 48) / DESK_W, (innerHeight - 110) / DESK_H, 1.5)
    : view.clientWidth / DESK_W
  frame.style.width = `${Math.round(DESK_W * scale)}px`
  frame.style.height = `${Math.round(DESK_H * scale)}px`
  stage.style.transform = `scale(${scale})`
}
window.addEventListener('resize', fitDesktop)
function desktopPanel(bot) {
  $('#panel-title').textContent = t('Desktop & apps')
  if (desk.bot !== bot.id || !$('#d-view')) { clearDesktopRecordingListeners(); giveBackDesktop(); desk.teaching=false; desktopRecording.queue=[];desktopRecording.token=null; desk.bot = bot.id; void connectDesktop() }
  // Built once: rebuilding on every panel visit would drop a typed address and flash the buttons.
  stableHtml($('#desktop-toolbar'), `<div class="desktop-toolbar-head"><span>${icon('desktop')}<strong>${t('Desktop & apps')}</strong></span><span class="visually-hidden" id="d-status" role="status"></span><button class="icon desktop-close" id="d-close" aria-label="${t('Close')}" title="${t('Close')}">✕</button></div>
    <div class="desktop-address"><span>${icon('browser')}</span><input id="d-url" type="text" inputmode="url" autocomplete="off" spellcheck="false" aria-label="${t('Open a website on the desktop')}" placeholder="${t('Enter a website to open on the desktop')}"><button class="ghost" id="d-go">${t('Open')}</button></div>
    <div class="desktop-apps" aria-label="${t('Desktop apps')}">${[['browser', 'browser', 'Browser'], ['files', 'memory', 'Files'], ['terminal', 'terminal', 'Terminal']].map(([app, glyph, label]) => `<button class="ghost" data-desktop-app="${app}">${icon(glyph)}${t(label)}</button>`).join('')}<button class="ghost" id="d-reload" title="${t('Reconnect')}">↻</button><button id="d-control"></button></div>`)
  $('#panel-body').innerHTML = `<p class="desktop-note muted" id="d-note"></p><details class="teach desktop-teach"><summary>桌面示范教学</summary><p class="muted">在 Bot 桌面中示范浏览器或软件操作，保存为带参数和验收步骤的技能。输入内容不会被录制。</p><div id="d-teach-controls"></div></details>`
  renderDesktopTeaching()
  $('#d-control').onclick = () => (desk.user ? giveBackDesktop() : takeDesktop())
  $('#d-reload').onclick = () => void connectDesktop()
  $('#d-close').onclick = () => closePanel()
  $('#d-go').onclick = () => void launchDesktopApp('browser', $('#d-url').value)
  $('#d-url').onkeydown = event => { if (event.key === 'Enter') { event.preventDefault(); void launchDesktopApp('browser', event.currentTarget.value) } }
  $('#desktop-toolbar').querySelectorAll('[data-desktop-app]').forEach(button => { button.onclick = () => void launchDesktopApp(button.dataset.desktopApp) })
  renderDesktopControl()
}
// The view holds the screen's proportions from the first frame, so nothing below it moves
// when the screen arrives. A new frame loads hidden under the waiting note and replaces the
// shown one only once it has loaded, so reconnecting never flashes an empty or white frame.
const desktopWaitHtml = (title, note) => `<div class="desktop-wait" id="d-wait"><span class="spinner"></span><strong>${title}</strong><p>${note}</p></div>`
async function connectDesktop() {
  const bot = desk.bot, ticket = ++desk.ticket
  desk.phase = 'starting'
  const shown = $('#d-view .desktop-stage iframe')
  if (shown) {
    $('#d-view').classList.remove('loaded')
    $('#d-view').classList.add('reconnecting')
    $('#d-wait').innerHTML = `<span class="spinner"></span><strong>${t('Reconnecting the desktop')}</strong>`
  } else {
    desk.observer?.disconnect(); desk.observer = null
    $('#desktop-host').innerHTML = `<div class="desktop-view" id="d-view">${desktopWaitHtml(t('Preparing your desktop'), t('The browser and apps will appear here. The first start may take a few minutes.'))}</div>`
  }
  renderDesktopControl()
  try {
    const url = await call('desktop.open', { bot })
    if (desk.bot !== bot || ticket !== desk.ticket) return
    const view = $('#d-view')
    if (!view.querySelector('.desktop-stage')) {
      view.innerHTML = `<div class="desktop-popbar"><span id="d-popnote"></span><button class="primary" id="d-giveback"></button></div>
        <div class="desktop-frame"><div class="desktop-stage" style="width:${DESK_W}px;height:${DESK_H}px"></div><div class="desktop-lock" id="d-lock"></div>${desktopWaitHtml(t('Connecting to the screen'), t('The browser and apps will appear here. The first start may take a few minutes.'))}</div>`
      $('#d-giveback').onclick = giveBackDesktop
      desk.observer = new ResizeObserver(fitDesktop); desk.observer.observe(view)
    }
    const frame = document.createElement('iframe')
    frame.src = url
    frame.title = t("{name}'s desktop", { name: botName(botById(bot)) ?? '' })
    frame.allow = 'clipboard-read; clipboard-write; fullscreen; autoplay'
    frame.className = 'incoming'
    let revealed = false
    const reveal = () => {
      if (ticket !== desk.ticket || desk.bot !== bot) { frame.remove(); return }
      if (revealed) return
      revealed = true
      for (const old of view.querySelectorAll('.desktop-stage iframe')) if (old !== frame) old.remove()
      frame.classList.remove('incoming')
      // "reconnecting" stays while the dimmed note fades out; switching its background now would flash black.
      view.classList.add('loaded')
      renderDesktopControl()
    }
    frame.addEventListener('load', () => { reveal(); if (desk.teaching) attachDesktopRecording() })
    // A screen page that never reports loading is still shown after a while.
    setTimeout(reveal, 20000)
    view.querySelector('.desktop-stage').append(frame)
    desk.phase = 'ready'
    renderDesktopControl()
  } catch (error) {
    if (desk.bot !== bot || ticket !== desk.ticket) return
    desk.phase = 'error'
    desk.observer?.disconnect(); desk.observer = null
    $('#d-view').classList.remove('loaded', 'popup')
    $('#d-view').innerHTML = `<div class="desktop-wait desktop-error"><strong>${t('The desktop could not connect')}</strong><p>${escape(t(error.message))}</p><button class="ghost" id="d-retry">${t('Retry')}</button></div>`
    $('#d-retry').onclick = () => void connectDesktop()
    renderDesktopControl()
  }
}
const launchDesktopApp = attempt(async (app, url = '') => {
  if (desk.launching || desk.phase !== 'ready') return
  if (app === 'browser' && url.trim()) {
    let parsed
    try { parsed = new URL(url.trim().includes('://') ? url.trim() : `https://${url.trim()}`) } catch { throw new Error(t('Enter a valid HTTP or HTTPS web address.')) }
    if (!['http:', 'https:'].includes(parsed.protocol) || !parsed.hostname || parsed.username || parsed.password) throw new Error(t('Enter a valid HTTP or HTTPS web address.'))
  }
  const bot = desk.bot
  desk.launching = true; renderDesktopControl()
  try {
    // Reserve the desktop before a shortcut opens a window, so the Bot cannot
    // click behind the user while the app is starting.
    await acquireDesktopControl(bot)
    if (desk.bot !== bot) return
    await call('desktop.launch', { bot, app, url })
    if (desk.bot === bot) { renderDesktopControl(); toast(t('Opened on the desktop. Use the same window to continue.')) }
  } finally { desk.launching = false; renderDesktopControl() }
})
function renderDesktopControl() {
  const name = botName(botById(desk.bot)) ?? t('The Bot')
  const frames = [...document.querySelectorAll('#d-view .desktop-stage iframe')], frame = frames.at(-1)
  for (const each of frames) { each.inert = !desk.user; each.tabIndex = desk.user ? 0 : -1 }
  const view = $('#d-view'), host = $('#desktop-host'), popup = desk.user && Boolean(frame)
  // Taken over, the screen floats above the page; its place in the panel keeps its height
  // and says where it went, so the notes below do not jump up and back.
  if (view && host) {
    if (popup && !view.classList.contains('popup')) host.style.setProperty('--desktop-slot', `${view.offsetHeight}px`)
    host.dataset.placeholder = t('{name}\'s screen is open above the page.', { name })
    host.classList.toggle('popped', popup)
  }
  view?.classList.toggle('locked', !desk.user)
  view?.classList.toggle('popup', popup)
  fitDesktop()
  const lock = $('#d-lock')
  if (lock) lock.innerHTML = `<span>🔒 ${escape(t('{name} is working here; you are watching.', { name }))}</span>`
  const status = $('#d-status')
  if (status) { status.textContent = t(desk.phase === 'starting' ? 'Starting desktop' : desk.phase === 'error' ? 'Connection failed' : desk.launching ? 'Opening app…' : desk.user ? 'You have control' : 'Watching the desktop'); status.dataset.phase = desk.phase }
  for (const button of document.querySelectorAll('#desktop-toolbar [data-desktop-app], #d-go, #d-control')) button.disabled = desk.phase !== 'ready'
  if ($('#d-popnote')) $('#d-popnote').textContent = t("You are using {name}'s desktop. {name} waits until you give it back.", { name })
  if ($('#d-giveback')) $('#d-giveback').textContent = t('Give back to {name}', { name })
  const button = $('#d-control')
  if (button) { button.textContent = desk.user ? t('Give back to {name}', { name }) : t('Take control'); button.className = desk.user ? 'ghost' : 'primary' }
  // Both notes share one grid cell, so the panel keeps the taller one's height either way.
  const note = $('#d-note')
  if (note) stableHtml(note, [
    [desk.user, t('You have control. {name} cannot click or type here until you give it back.', { name })],
    [!desk.user, t('All Bots share software and files on one computer, with an independent screen for each Bot. Take control to use this screen; {name} waits until you give it back.', { name })],
  ].map(([on, text]) => `<span ${on ? '' : 'class="off" aria-hidden="true"'}>${escape(text)}</span>`).join(''))
  if (desk.user && frame) frame.focus()
}
async function acquireDesktopControl(bot) {
  await call('desktop.control', { bot, user: true })
  if (desk.bot !== bot) { await call('desktop.control', { bot, user: false }); return }
  desk.user = true
  if (desk.teaching) attachDesktopRecording()
  clearInterval(desk.renew)
  desk.renew = setInterval(() => { call('desktop.control', { bot, user: true }).catch(error => { if (desk.bot !== bot) return; clearInterval(desk.renew); desk.user = false; renderDesktopControl(); toast(t(error.message)) }) }, 20000)
  renderDesktopControl()
}
const takeDesktop = attempt(async () => { if (desk.phase === 'ready') await acquireDesktopControl(desk.bot) })
function giveBackDesktop() {
  clearDesktopRecordingListeners()
  if (!desk.user) return
  clearInterval(desk.renew)
  desk.user = false
  const bot=desk.bot
  let pending=Promise.resolve()
  if(desk.teaching) { while(desktopRecording.queue.length)pending=flushDesktopRecording(); pending=desktopRecording.sending }
  pending.catch(error=>toast(t(error.message))).finally(()=>call('desktop.control', { bot, user: false }).catch(() => {}))
  if (document.activeElement?.tagName === 'IFRAME') document.activeElement.blur()
  renderDesktopControl()
}
// A closed tab gives the controls back at once rather than when the lease runs out.
window.addEventListener('pagehide', () => {
  if (desk.user) navigator.sendBeacon('/api/bot', new Blob([JSON.stringify({ method: 'desktop.control', params: { bot: desk.bot, user: false } })], { type: 'application/json' }))
})

const EFFORT_LEVELS = [['none', 'Off'], ['minimal', 'Minimal'], ['low', 'Low'], ['medium', 'Medium'], ['high', 'High'], ['xhigh', 'Extra high'], ['max', 'Maximum']]
const effortLabel = key => t((EFFORT_LEVELS.find(([k]) => k === key) ?? [])[1] ?? 'Provider default')
// `first` is the label of the empty choice: what applies when nothing is chosen here.
function effortOptions(value = '', first = t('Provider default')) {
  return [['', first], ...EFFORT_LEVELS.map(([key, label]) => [key, t(label)])].map(([key, label]) => `<option value="${key}" ${value === key ? 'selected' : ''}>${escape(label)}</option>`).join('')
}
// A Bot's own folder only matters while the local terminal is on; empty falls back to
// the team workspace as it is now, so the hint follows that setting when it changes.
function syncWorkDirField() {
  const field = $('#f-dir-field')
  if (!field) return
  field.hidden = !view.team.localComputer?.enabled
  field.querySelector('label').textContent = t('Working folder (full path; empty: the team workspace {path})', { path: view.team.workspace ?? '' })
  $('#f-dir').placeholder = view.team.workspace ?? ''
}
function botDetails(bot) {
  $('#panel-title').textContent = botName(bot)
  const permissions = [['ask', 'Ask me before risky steps'], ['auto', 'AI approves; ask me only for irreversible steps'], ['edits', 'Ask before commands and app launches'], ['readonly', 'Read only']]
  $('#panel-body').innerHTML = `
    <div class="panel-hero">${avatar(bot)}<div><strong>${escape(botName(bot))}</strong><span class="muted">${escape([botRole(bot), bot.main ? t('Main Bot') : ''].filter(Boolean).join(' · ') || t('Bot'))}</span></div></div>
    <section class="panel-section"><h4>${t('Basics')}</h4><div class="panel-grid">
      <div><label for="f-name">${t('Name')}</label><input id="f-name" value="${escape(bot.name)}"></div>
      <div><label for="f-role">${t('Role')}</label><input id="f-role" value="${escape(bot.role)}"></div>
      <div class="full"><label for="f-group">${t('Sidebar group')}</label><select id="f-group">${groupOptions(bot.sidebarGroup)}</select></div></div></section>
    <section class="panel-section"><h4>${t('Duties & instructions')}</h4>
      <label for="f-description">${t('What it is for (other Bots read this to hand it work)')}</label><textarea id="f-description" rows="2">${escape(bot.description ?? '')}</textarea>
      <label for="f-instructions">${t('Instructions')}</label><textarea id="f-instructions" rows="6">${escape(bot.instructions)}</textarea></section>
    <section class="panel-section"><h4>${t('Model & permissions')}</h4>
      <label for="f-model">${t('Model (empty: the provider\'s default for chats)')}</label><input id="f-model" list="provider-models" value="${escape(bot.model ?? '')}" placeholder="${escape(view.team.provider?.defaultModel || t('default'))}">${modelList()}<p class="muted field-hint">${t('On {name}', { name: escape(view.team.provider?.name ?? '') })}</p>
      <label for="f-effort">${t('Reasoning effort')}</label><select id="f-effort">${effortOptions(bot.effort ?? '', view.team.provider?.defaultEffort ? t('Follow the provider ({level})', { level: effortLabel(view.team.provider.defaultEffort) }) : t('Provider default'))}</select><p class="muted field-hint">${view.team.provider?.defaultEffort ? t('Empty follows {name}\'s default reasoning effort; choose a level to override it for this Bot. Support varies by model.', { name: escape(view.team.provider.name) }) : t('Support varies by model. Default sends no reasoning setting; higher effort may take longer.')}</p>
      <label for="f-perm">${t('Permissions')}</label><select id="f-perm">${permissions.map(([v, label]) => `<option value="${v}" ${(bot.permission ?? 'auto') === v ? 'selected' : ''}>${t(label)}</option>`).join('')}</select>
      <div id="f-dir-field"><label for="f-dir"></label><input id="f-dir" value="${escape(bot.workDir ?? '')}"></div></section>
    <div class="panel-actions"><button class="ghost" id="f-reset">${t('New conversation')}</button><button class="primary" id="f-save">${t('Save')}</button></div>
    <section class="panel-section panel-more"><h4>${t('More')}</h4><div class="form-actions"><button class="ghost" id="f-pin">${bot.pinned ? t('Unpin') : t('Pin to top')}</button><button class="ghost" id="f-main">${bot.main ? t('Not a Main Bot') : t('Make Main Bot')}</button><button class="danger" id="f-delete">${t('Delete')}</button></div></section>`
  syncWorkDirField()
  $('#f-save').onclick = attempt(async () => {
    // Only changed fields are sent: an unchanged name, role or instructions must not start a
    // fresh conversation just because another option was saved.
    const form = { name: $('#f-name').value.trim(), role: $('#f-role').value.trim(), description: $('#f-description').value.trim(), instructions: $('#f-instructions').value.trim(), model: $('#f-model').value.trim(), effort: $('#f-effort').value, workDir: $('#f-dir')?.value.trim() ?? (bot.workDir ?? ''), permission: $('#f-perm').value }
    const saved = { name: bot.name, role: bot.role, description: bot.description ?? '', instructions: bot.instructions ?? '', model: bot.model ?? '', effort: bot.effort ?? '', workDir: bot.workDir ?? '', permission: bot.permission ?? 'auto' }
    const patch = Object.fromEntries(Object.entries(form).filter(([key, value]) => JSON.stringify(value) !== JSON.stringify(saved[key])))
    if (Object.keys(patch).length) await call('bot.update', { id: bot.id, ...patch })
    await call('sidebar.move', { conversation: bot.conversation, group: $('#f-group').value })
    toast(t('Saved'))
  })
  $('#f-reset').onclick = attempt(async () => { await call('chat.reset', { conversation: bot.conversation }); await open(bot.conversation) })
  $('#f-pin').onclick = attempt(async () => { await call('bot.pin', { id: bot.id, pinned: !bot.pinned }) })
  $('#f-main').onclick = attempt(async () => { await call('bot.setMain', { id: bot.id, main: !bot.main }) })
  $('#f-delete').onclick = attempt(async () => {
    if (!confirm(t('Delete {name}? Its memory moves to the archive.', { name: bot.name }))) return
    await call('bot.delete', { id: bot.id })
    view.current = null
    closePanel()
    renderAll()
  })
}

function roomDetails(room) {
  $('#panel-title').textContent = room.name
  $('#panel-body').innerHTML = `
    <div class="panel-hero">${roomAvatar(room)}<div><strong>${escape(room.name)}</strong><span class="muted">${escape(t('{n} members', { n: room.members.length }))}</span></div></div>
    <section class="panel-section"><h4>${t('Basics')}</h4>
    <label for="r-name">${t('Name')}</label><input id="r-name" value="${escape(room.name)}">
    <label for="r-group">${t('Sidebar group')}</label><select id="r-group">${groupOptions(room.sidebarGroup)}</select>
    <label for="r-notice">${t('Notice')}</label><textarea id="r-notice" rows="3">${escape(room.notice)}</textarea></section>
    <section class="panel-section"><h4>${t('Members')}</h4><div class="checks member-checks">${bots().map(bot => `<label>${avatar(bot, 'tiny')}<span class="member-name">${escape(bot.name)}</span><input type="checkbox" value="${escape(bot.id)}" ${room.members.includes(bot.id) ? 'checked' : ''}></label>`).join('')}</div>
    <label for="r-admin">${t('Admin')}</label><select id="r-admin"><option value="">${t('None')}</option>${room.members.map(id => `<option value="${escape(id)}" ${room.admin === id ? 'selected' : ''}>${escape(botById(id)?.name ?? id)}</option>`).join('')}</select>
    <p class="muted field-hint">${t('Messages without @ are claimed by the members they concern; @Name calls a member, @all everyone.')}</p></section>
    <div class="panel-actions"><button class="danger" id="r-delete">${t('Delete group')}</button><button class="primary" id="r-save">${t('Save')}</button></div>`
  $('#r-save').onclick = attempt(async () => {
    const members = [...document.querySelectorAll('#panel-body .checks input:checked')].map(el => el.value)
    await call('room.update', { id: room.id, name: $('#r-name').value, notice: $('#r-notice').value, members, admin: $('#r-admin').value || null, mode: 'auto' })
    await call('sidebar.move', { conversation: room.conversation, group: $('#r-group').value })
    toast(t('Saved'))
  })
  $('#r-delete').onclick = attempt(async () => {
    if (!confirm(t('Delete {name}? Its memory moves to the archive.', { name: room.name }))) return
    await call('room.delete', { id: room.id })
    view.current = null
    closePanel()
    renderAll()
  })
}

// Where an entry came from and when, in plain words: "自动整理 · 2026-10-24".
function memoryMeta(meta = []) {
  const get = key => meta.find(([k]) => k === key)?.[1] ?? ''
  const source = get('source'), added = get('added') || get('updated')
  const from = !source ? '' : source === 'user' ? t('Added by you') : source === 'feedback' ? t('From reply feedback') : /^memory review/.test(source) ? t('Organized automatically') : /^part \d+/.test(source) ? t('From a conversation') : source
  const rest = meta.filter(([k]) => !['source', 'added', 'updated'].includes(k)).map(([k, v]) => `${k}: ${v}`)
  return [from, added, ...rest].filter(Boolean).join(' · ')
}
// Search narrows every memory list in the panel; picked entries go in one batch.
let memoryQuery = ''
function bindMemoryList(ticket) {
  const box = $('#panel-body'), picks = () => [...box.querySelectorAll('.mem-pick:checked')]
  const shown = () => [...box.querySelectorAll('.mem-entry:not([hidden]) .mem-pick')]
  const sync = () => {
    const n = picks().length
    $('#m-bulk').hidden = !n
    $('#m-picked').textContent = t('{n} selected', { n })
    box.classList.toggle('mem-picking', n > 0)
  }
  const filter = () => {
    const words = memoryQuery.toLowerCase().split(/\s+/).filter(Boolean)
    let found = 0
    for (const entry of box.querySelectorAll('.mem-entry')) { const hit = words.every(w => entry.dataset.search.includes(w)); entry.hidden = !hit; if (hit) found++ }
    for (const group of box.querySelectorAll('.mem-group')) group.hidden = Boolean(words.length) && !group.querySelector('.mem-entry:not([hidden])')
    $('#m-found').textContent = words.length ? t('{n} found', { n: found }) : ''
    box.querySelector('.mem-nomatch').hidden = !words.length || found > 0
  }
  $('#m-search').oninput = () => { memoryQuery = $('#m-search').value; filter() }
  box.querySelectorAll('.mem-pick').forEach(el => { el.onchange = sync })
  $('#m-pick-all').onclick = () => { shown().forEach(el => { el.checked = true }); sync() }
  $('#m-pick-none').onclick = () => { picks().forEach(el => { el.checked = false }); sync() }
  $('#m-forget-picked').onclick = attempt(async () => {
    const items = picks().map(el => JSON.parse(el.dataset.pick))
    if (!items.length || !confirm(t('Delete {n} memory entries? This cannot be undone.', { n: items.length }))) return
    const result = await call('memory.forget_many', { items })
    toast(result.failed.length ? t('Deleted {n}; {m} could not be deleted', { n: result.removed, m: result.failed.length }) : t('Deleted {n} entries', { n: result.removed }))
    if (ticket === panelTicket) openPanel('memory')
  })
  filter(); sync()
}
const memoryPanel = async (bot, room) => {
  const ticket = panelTicket, identity = bot?.id ?? room?.id, box = $('#panel-body')
  if (box.dataset.memoryIdentity !== identity || !$('#m-ask')) stableHtml(box, skeletonHtml('settings'))
  box.dataset.memoryIdentity = identity
  $('#panel-title').textContent = t('Memory')
  const scopes = bot ? [{ scope: 'bot', id: bot.id, title: t("{name}'s memory", { name: bot.name }) }, { scope: 'team', title: t('Team memory') }] : [{ scope: 'group', id: room.id, title: t("{name}'s memory", { name: room.name }) }]
  let views
  try { views = await Promise.all(scopes.map(s => call('memory.view', { scope: s.scope, id: s.id }, { signal: AbortSignal.timeout(15000) }))) }
  catch {
    if (ticket === panelTicket && view.panel === 'memory') { box.setAttribute('aria-busy', 'false'); stableHtml(box, loadErrorHtml('data-memory-retry')) }
    return
  }
  if (ticket !== panelTicket || view.panel !== 'memory' || identity !== (currentBot()?.id ?? currentRoom()?.id)) return
  box.setAttribute('aria-busy', 'false')
  const first = { scope: scopes[0].scope, id: scopes[0].id }
  const count = v => v.files.reduce((n, file) => n + file.entries.length, 0)
  $('#panel-body').innerHTML = `<div class="mem-ask"><div class="mem-ask-bar"><input id="m-ask" placeholder="${t('Ask or change: “what do you know about my trip?”, “forget the old address”')}"><button class="primary" id="m-ask-go">${t('Ask')}</button></div><div id="m-answer" class="muted"></div>
</div>`
    + `<div class="mem-tools"><input id="m-search" type="search" value="${escape(memoryQuery)}" placeholder="${t('Search memory…')}" aria-label="${t('Search memory…')}"><span id="m-found" class="muted" aria-live="polite"></span></div>
    <div class="mem-bulk" id="m-bulk" hidden><span id="m-picked"></span><button class="link" id="m-pick-all">${t('Select all shown')}</button><button class="link" id="m-pick-none">${t('Clear selection')}</button><button class="danger" id="m-forget-picked">${t('Delete selected')}</button></div>`
    + scopes.map((s, index) => `<section class="panel-section mem-file"><h4>${escape(s.title)}<span class="mem-count">${count(views[index])}</span></h4>
    ${views[index].files.map(file => `<div class="mem-group"><div class="mem-topic">${escape(file.topic ? file.topic : t('Always remembered'))}<span class="muted">${t('{n} chars', { n: file.size })}</span></div>
      ${file.entries.map(entry => { const ref = escape(JSON.stringify({ scope: s.scope, id: s.id, topic: file.topic, entry: entry.text })); return `<div class="mem-entry" data-search="${escape(`${entry.text} ${memoryMeta(entry.meta)}`.toLowerCase())}"><input type="checkbox" class="mem-pick" data-pick='${ref}' aria-label="${t('Select')}"><div class="t">${escape(entry.text)}${memoryMeta(entry.meta) ? `<div class="m">${escape(memoryMeta(entry.meta))}</div>` : ''}</div>
        <button class="icon" title="${t('Forget')}" aria-label="${t('Forget')}" data-forget='${ref}'>✕</button></div>` }).join('') || `<div class="mem-empty muted">${t('Nothing saved yet.')}</div>`}</div>`).join('') || `<div class="mem-empty muted">${t('Nothing saved yet.')}</div>`}<div class="mem-empty muted mem-nomatch" hidden>${t('No matching memory.')}</div>
    <div class="mem-add"><input placeholder="${t('Add an entry…')}" data-add='${escape(JSON.stringify({ scope: s.scope, id: s.id }))}'><button class="ghost" data-add-go>${t('Add')}</button></div></section>`).join('')
  $('#m-ask-go').onclick = attempt(async () => {
    const text = $('#m-ask').value.trim()
    if (!text) return
    $('#m-answer').textContent = t('Asking the model…')
    const answer = await call('memory.ask', { ...first, text })
    if (ticket !== panelTicket) return
    $('#m-answer').textContent = [answer.reply, ...answer.changes].filter(Boolean).join(' · ')
    if (answer.changes.length) setTimeout(() => { if (ticket === panelTicket) openPanel('memory') }, 1200)
  })
  $('#m-ask').onkeydown = (event) => { if (event.key === 'Enter') $('#m-ask-go').click() }
  bindMemoryList(ticket)
}

// The live browser: the Bot's own browser, refreshed while the panel is open; a click
// on the picture clicks the page there.
function stopLive() {
  clearTimeout(view.live?.timer)
  if (view.live?.user) call('browser.control', { bot: view.live.bot, user: false }).catch(() => {})
  view.live = null; browserKeys = null
}
// The Bot's browser: a locked live thumbnail while the Bot browses; taking control opens it
// in a popup where the picture takes clicks, the wheel scrolls and keys go to the page.
function browserPanel(bot) {
  $('#panel-title').textContent = t("{name}'s browser", { name: bot.name })
  const teaching = view.work.teaching?.includes(bot.id)
  const user = view.live?.bot === bot.id && view.live.user
  $('#panel-body').innerHTML = `
    <div class="browser-card ${user ? 'popup' : ''}" id="b-card">
      <div class="b-popbar"><span>${escape(t("You are using {name}'s browser. Click the picture, type and scroll as on a page.", { name: bot.name }))}</span><button class="primary" id="b-giveback">${escape(t('Give back to {name}', { name: bot.name }))}</button></div>
      <div class="b-window">
        <div class="browser-bar b-tools"><button class="ghost" id="b-back" title="${t('Back')}">←</button><input id="b-url" placeholder="${t('Address')}"><button class="ghost" id="b-go">${t('Go')}</button></div>
        <div class="b-where"><span class="b-dot"></span><span id="b-where">${t('Starting the browser…')}</span></div>
        <div class="browser-view live" id="b-view"><div class="browser-empty"><span class="spinner"></span><p>${t('Starting the browser… The first start can take up to a minute.')}</p></div></div>
        <div class="b-lock"><span>🔒 ${escape(t('{name} is working here; you are watching.', { name: bot.name }))}</span></div>
        <div class="browser-bar b-tools"><input id="b-type" placeholder="${t('Type into the page')}"><button class="ghost" id="b-send">${t('Type')}</button><button class="ghost" id="b-enter">⏎</button><button class="ghost" id="b-up" title="${t('Scroll up')}">↑</button><button class="ghost" id="b-down" title="${t('Scroll down')}">↓</button></div>
      </div>
    </div>
    <div class="browser-bar"><button class="primary" id="b-take">${t('Take control')}</button><button class="ghost" id="b-share" title="${t("Copy this browser's logins to every Bot's browser")}">${t('Share logins')}</button></div>
    <p class="muted">${escape(t('{name} browses here by itself and every step shows in the chat. Take control to sign in to a site, pass a human check or show it something.', { name: bot.name }))}</p>
    <details class="teach ${teaching ? 'on' : ''}" ${teaching ? 'open' : ''}><summary>${t('Teach a task')}</summary>${teaching
      ? `<strong>${t('Teaching {name}', { name: escape(bot.name) })}</strong><p class="muted">${t('Do the task here, step by step. Then say what it is for.')}</p><input id="b-goal" placeholder="${t('What this task is for')}"><div class="form-actions"><button class="ghost" id="b-cancel">${t('Cancel')}</button><button class="primary" id="b-finish">${t('Save as a skill')}</button></div>`
      : `<strong>${t('Teach a task')}</strong><button class="ghost" id="b-teach">${t('Start teaching')}</button>`}</details>`
  // One step at a time, in order: a click sent while text is still being typed would land first.
  let steps = Promise.resolve()
  const act = action => { steps = steps.then(attempt(async () => { await call('browser.act', { bot: bot.id, ...action }); refreshLive(bot, 250) })) }
  const take = attempt(async () => { await call('browser.control', { bot: bot.id, user: true }); if (view.live?.bot !== bot.id) return; view.live.user = true; $('#b-card')?.classList.add('popup'); refreshLive(bot, 0) })
  $('#b-take').onclick = take
  $('#b-giveback').onclick = giveBackBrowser
  $('#b-go').onclick = () => { const url = $('#b-url').value.trim(); if (url) act({ kind: 'navigate', url }) }
  $('#b-url').onkeydown = (event) => { if (event.key === 'Enter') $('#b-go').click() }
  $('#b-back').onclick = () => act({ kind: 'back' })
  const typeText = submit => { const text = $('#b-type').value; if (text) { $('#b-type').value = ''; act({ kind: 'type', text, submit }) } else if (submit) act({ kind: 'key', key: 'Enter' }) }
  $('#b-send').onclick = () => typeText(false)
  $('#b-type').onkeydown = (event) => { if (event.key === 'Enter' && !event.isComposing) typeText(true) }
  $('#b-enter').onclick = () => act({ kind: 'key', key: 'Enter' })
  $('#b-up').onclick = () => act({ kind: 'scroll', dy: -500 })
  $('#b-down').onclick = () => act({ kind: 'scroll', dy: 500 })
  $('#b-share').onclick = attempt(async () => { toast(t(await call('browser.share', { bot: bot.id }))) })
  if (teaching) {
    $('#b-cancel').onclick = attempt(async () => { await call('browser.teach', { bot: bot.id, action: 'cancel' }) })
    $('#b-finish').onclick = attempt(async () => { await call('browser.teach', { bot: bot.id, action: 'finish', goal: $('#b-goal').value }); toast(t('{name} is writing the skill', { name: bot.name })) })
  } else $('#b-teach').onclick = attempt(async () => { await call('browser.teach', { bot: bot.id, action: 'start' }); take() })
  // Only a taken-over browser takes the pointer and keys.
  const taken = () => $('#b-card')?.classList.contains('popup')
  $('#b-view').onclick = (event) => {
    const img = event.target.closest('img')
    if (!img || !taken()) return
    const rect = img.getBoundingClientRect()
    const x = Math.round((event.clientX - rect.left) * img.naturalWidth / rect.width)
    const y = Math.round((event.clientY - rect.top) * img.naturalHeight / rect.height)
    act({ kind: 'click', x, y, label: $('#b-where').textContent })
  }
  let wheel = 0, wheelTimer
  $('#b-view').onwheel = (event) => {
    if (!taken()) return
    event.preventDefault()
    wheel += event.deltaY
    clearTimeout(wheelTimer)
    wheelTimer = setTimeout(() => { const dy = Math.round(wheel); wheel = 0; if (dy) act({ kind: 'scroll', dy }) }, 120)
  }
  // Keys pressed over the popup go to the page; letters are gathered into one typing step.
  let typed = '', typeTimer
  browserKeys = (event) => {
    if (!taken() || event.target.closest?.('input, textarea') || event.ctrlKey || event.metaKey || event.altKey || event.isComposing) return
    const special = ['Enter', 'Backspace', 'Tab', 'Delete', 'Escape', 'ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight', 'Home', 'End', 'PageUp', 'PageDown']
    if (event.key.length === 1) { typed += event.key; clearTimeout(typeTimer); typeTimer = setTimeout(() => { const text = typed; typed = ''; act({ kind: 'type', text }) }, 250) }
    else if (special.includes(event.key)) { if (typed) { clearTimeout(typeTimer); act({ kind: 'type', text: typed }); typed = '' } act({ kind: 'key', key: event.key }) }
    else return
    event.preventDefault()
  }
  if (!view.live || view.live.bot !== bot.id) view.live = { bot: bot.id, user: false }
  refreshLive(bot, 0)
}
let browserKeys = null
document.addEventListener('keydown', event => browserKeys?.(event))
function giveBackBrowser() {
  if (view.live?.user) call('browser.control', { bot: view.live.bot, user: false }).catch(error => toast(error.message))
  if (view.live) view.live.user = false
  $('#b-card')?.classList.remove('popup')
}

function refreshLive(bot, delay) {
  if (!view.live || view.live.bot !== bot.id) return
  clearTimeout(view.live.timer)
  view.live.timer = setTimeout(async () => {
    if (!view.live || view.live.bot !== bot.id) return
    if (!document.hidden) {
      try {
        if (view.live?.user) await call('browser.control', { bot: bot.id, user: true })
        const peek = await call('browser.peek', { bot: bot.id })
        const box = $('#b-view')
        if (box && view.live?.bot === bot.id) {
          if (peek.image) {
            // Keep the same img: a new one between mouse down and up swallows the click.
            const img = box.querySelector('img')
            if (img) img.src = peek.image
            else box.innerHTML = `<img draggable="false" src="${peek.image}" alt="${escape(t("{name}'s browser", { name: bot.name }))}" title="${t('Click to click the page')}">`
            $('#b-where').textContent = [peek.title, peek.url].filter(Boolean).join(' · ')
            if (peek.url && document.activeElement?.id !== 'b-url') $('#b-url').value = peek.url
            let banner = $('#b-challenge')
            if (peek.challenge && !banner) box.insertAdjacentHTML('beforebegin', `<div class="challenge-banner" id="b-challenge">🛡 ${escape(t('This page wants a human check. Click and type in the picture below to complete it yourself; {name} waits.', { name: bot.name }))}</div>`)
            else if (!peek.challenge && banner) banner.remove()
          } else {
            // A blank tab shows as a white picture; say what to do instead.
            const text = peek.blank
              ? t('Nothing is open in this browser yet. Type an address above, or ask {name} to open a page.', { name: bot.name })
              : t('No picture from the browser: {problem}', { problem: t(peek.problem || '—') })
            if (box.dataset.empty !== text) { box.dataset.empty = text; box.innerHTML = `<div class="browser-empty"><span>🌐</span><p>${escape(text)}</p></div>` }
            $('#b-where').textContent = ''
            $('#b-url').value = ''
          }
          if (peek.image) delete box.dataset.empty
        }
      } catch (error) {
        const box = $('#b-view')
        if (box) box.innerHTML = `<span class="muted">${escape(t(error.message))}</span>`
      }
    }
    refreshLive(bot, view.live?.user ? 600 : 1500)
  }, delay)
}

function settingsModal() { openPage('general') }

function renderGeneralSettings() {
  const p = profile()
  $('#settings-content').innerHTML = `<div class="page general-settings">
    <h4>${t('Your profile')}</h4>
    <section class="profile-card" id="admin-settings"><div class="profile-row"><button type="button" class="profile-photo" id="p-photo" title="${t('Change photo')}" aria-label="${t('Change photo')}"><span id="p-avatar">${userAvatar()}</span></button>
      <div class="profile-identity"><div><strong id="p-display">${escape(p.name ?? '')}</strong><span class="badge">${t('Administrator')}</span></div><small class="muted">${t('Sign-in username')} <span id="admin-identity">…</span></small>
      <div class="profile-avatar-actions"><button type="button" class="link-button" id="p-pick">${t('Change photo')}</button>${p.avatar ? `<button type="button" class="link-button" id="p-clear">${t('Remove')}</button>` : ''}<input type="file" id="p-file" accept="image/png,image/jpeg,image/gif,image/webp" hidden></div></div>
      <button class="ghost" id="admin-logout" type="button" hidden>${t('Sign out')}</button></div>
      <div class="profile-fields"><label>${t('Your name')}<input id="p-name" maxlength="40" value="${escape(p.name ?? '')}" placeholder="${t('What Bots call you')}"></label>
        <label>${t('Birthday')}<input id="p-birthday" type="date" value="${escape(p.birthday ?? '')}"></label>
        <label class="full">${t('About you')}<textarea id="p-bio" rows="4" maxlength="500" placeholder="${t('What you do, what you like, how you want Bots to work with you')}">${escape(p.bio ?? '')}</textarea><small class="muted profile-count" id="p-bio-count">${(p.bio ?? '').length}/500</small></label></div>
      <div id="admin-credentials"></div></section>

    <h4>${t('Local terminal')}</h4>
    <section class="local-settings"><label class="local-toggle" for="s-local"><span class="local-terminal-symbol">${icon('terminal')}</span><span class="grow"><strong>${t('Enable local terminal collaboration')}</strong></span><input type="checkbox" role="switch" id="s-local" ${view.team.localComputer?.enabled ? 'checked' : ''}></label>
    <div id="s-local-body" ${view.team.localComputer?.enabled ? '' : 'hidden'}><div class="local-status-row"><div id="s-local-state"></div><button class="ghost" id="s-local-pause"></button></div></div></section>
    <h4>${t('Team')}</h4>
    <section class="settings-card"><label>${t('Team workspace: where Bots without a folder of their own work')}<input id="s-ws" value="${escape(view.team.workspace)}" spellcheck="false"></label>
    <div class="settings-grid"><label>${t('Appearance')}<select id="s-theme">${[['system', 'Follow the system'], ['light', 'Light'], ['dark', 'Dark']].map(([v, n]) => `<option value="${v}" ${themeChoice() === v ? 'selected' : ''}>${t(n)}</option>`).join('')}</select></label>
    <label>${t('Language')}<select id="s-lang"><option value="en" ${LANG === 'en' ? 'selected' : ''}>English</option><option value="zh" ${LANG === 'zh' ? 'selected' : ''}>中文</option></select></label>
    <label>${t('Notifications')}<button class="ghost" id="s-notify" type="button">${typeof Notification !== 'undefined' && Notification.permission === 'granted' ? t('On') : t('Turn on desktop notifications')}</button></label></div></section>
    <div class="form-actions settings-save"><button class="ghost" data-settings-close>${t('Back to chat')}</button><button class="primary" id="s-save">${t('Save')}</button></div></div>`
  void renderAdminSettings($('#admin-settings'))
  renderLocalStatus()
  $('#s-local').onchange = attempt(async () => {
    const toggle = $('#s-local'); toggle.disabled = true
    try { view.team.localComputer = await call('settings.local-computer', { enabled: toggle.checked }); renderLocalStatus(); syncWorkDirField() }
    catch (error) { toggle.checked = Boolean(view.team.localComputer?.enabled); throw error }
    finally { toggle.disabled = false }
  })
  $('#s-lang').onchange = (event) => { setLang(event.target.value) }
  $('#s-theme').onchange = (event) => setTheme(event.target.value)
  let avatar
  $('#p-pick').onclick = $('#p-photo').onclick = () => $('#p-file').click()
  $('#p-bio').oninput = () => { $('#p-bio-count').textContent = `${$('#p-bio').value.length}/500` }
  $('#p-name').oninput = () => { $('#p-display').textContent = $('#p-name').value.trim() || $('#p-display').dataset.fallback || '' }
  $('#p-file').onchange = () => {
    const file = $('#p-file').files[0]
    if (!file) return
    shrinkImage(file, 256).then(url => { avatar = url; $('#p-avatar').innerHTML = `<img class="avatar user-avatar" src="${url}" alt="">` }).catch(error => toast(error.message))
  }
  if ($('#p-clear')) $('#p-clear').onclick = () => { avatar = ''; $('#p-avatar').innerHTML = `<span class="avatar user-avatar">${escape(($('#p-name').value || t('You')).slice(0, 1).toUpperCase())}</span>` }
  $('#s-save').onclick = attempt(async () => {
    const params = { name: $('#p-name').value, bio: $('#p-bio').value, birthday: $('#p-birthday').value }
    if (avatar !== undefined) params.avatar = avatar
    await call('profile.update', params)
    if ($('#s-ws').value !== view.team.workspace) { view.team.workspace = await call('settings.workspace', { path: $('#s-ws').value }); syncWorkDirField() }
    closeSettings(); toast(t('Saved'))
  })
  $('#s-notify').onclick = attempt(async () => { if (typeof Notification !== 'undefined') { await Notification.requestPermission(); renderGeneralSettings() } })
}

// A picture as a square JPEG data URL at most `size` px wide, so an avatar stays small.
function shrinkImage(file, size) {
  return new Promise((resolve, reject) => {
    const img = new Image()
    img.onload = () => {
      const side = Math.min(img.width, img.height)
      const out = Math.min(size, side)
      const canvas = Object.assign(document.createElement('canvas'), { width: out, height: out })
      canvas.getContext('2d').drawImage(img, (img.width - side) / 2, (img.height - side) / 2, side, side, 0, 0, out, out)
      URL.revokeObjectURL(img.src)
      resolve(canvas.toDataURL('image/jpeg', 0.9))
    }
    img.onerror = () => reject(new Error(t('Not an image')))
    img.src = URL.createObjectURL(file)
  })
}

function modal(html, wide = false) {
  detailOpen = null
  $('#modal-card').innerHTML = html
  $('#modal-card').classList.toggle('wide', wide)
  $('#modal').hidden = false
}
function closeModal() { detailOpen = null; $('#modal').hidden = true }

function sidebarGroupModal(name = '') {
  const entries = [...bots().map(b => ({ ...b, label: botName(b), room: false })), ...rooms().map(r => ({ ...r, label: r.name, room: true }))]
  modal(`<h3>${t(name ? 'Edit sidebar group' : 'New sidebar group')}</h3>
    
    <label for="sg-name">${t('Name')}</label><input id="sg-name" maxlength="32" autocomplete="off" placeholder="${t('e.g. Product launch')}" value="${escape(name)}">
    <div class="sg-members-head"><label for="sg-filter">${t('Conversations')}</label><span id="sg-count" class="muted"></span></div>
    ${entries.length > 6 ? `<input id="sg-filter" type="search" autocomplete="off" placeholder="${t('Search conversations')}">` : ''}
    <div class="sidebar-group-members">${entries.map(entry => `<label class="sg-member" data-search="${escape(entry.label.toLocaleLowerCase())}">${entry.room ? roomAvatar(entry) : avatar(entry)}<span class="grow"><strong>${escape(entry.label)}</strong><small>${entry.room ? t('Group chat') : t('Bot')}${entry.pinned ? ` · ${t('Pinned')}` : ''}${entry.sidebarGroup && entry.sidebarGroup !== name ? ` · ${escape(t('Now in {group}', { group: entry.sidebarGroup }))}` : ''}</small></span><input type="checkbox" value="${escape(entry.conversation)}" ${name && entry.sidebarGroup === name ? 'checked' : ''}></label>`).join('') || `<div class="group-empty">${t('No conversations yet.')}</div>`}</div>
    <div class="form-actions">${name ? `<button class="danger" id="sg-delete">${t('Delete sidebar group')}</button>` : '<span></span>'}<span class="form-actions-end"><button class="ghost" data-close>${t('Cancel')}</button><button class="primary" id="sg-save">${t(name ? 'Save' : 'Create')}</button></span></div>`)
  const update = () => {
    const picked = document.querySelectorAll('.sidebar-group-members input:checked').length
    $('#sg-count').textContent = picked ? t('{n} selected', { n: picked }) : t('Optional')
    $('#sg-save').disabled = !$('#sg-name').value.trim()
  }
  $('#sg-name').oninput = update
  $('.sidebar-group-members').onchange = update
  if ($('#sg-filter')) $('#sg-filter').oninput = () => {
    const query = $('#sg-filter').value.trim().toLocaleLowerCase()
    for (const row of document.querySelectorAll('.sg-member')) row.hidden = Boolean(query) && !row.dataset.search.includes(query)
  }
  update()
  $('#sg-name').focus()
  $('#sg-save').onclick = attempt(async () => {
    const newName = $('#sg-name').value.trim()
    if (!newName) return
    const conversations = [...document.querySelectorAll('.sidebar-group-members input:checked')].map(el => el.value)
    await call(`sidebar.group.${name ? 'update' : 'create'}`, { name: name || newName, newName, conversations })
    closeModal(); await refresh()
  })
  $('#sg-name').onkeydown = event => { if (event.key === 'Enter' && !event.isComposing) { event.preventDefault(); $('#sg-save').click() } }
  if (name) $('#sg-delete').onclick = attempt(async () => {
    if (!confirm(t('Delete this sidebar group? Conversations move to Ungrouped.'))) return
    await call('sidebar.group.delete', { name }); closeModal(); await refresh()
  })
}

function newBotModal() {
  modal(`<h3>${t('New Bot')}</h3>
    <label>${t('Start from a role')}</label><select id="n-template"><option value="">${t('Custom Bot')}</option>${(view.team.templates ?? []).map(template => `<option value="${escape(template.id)}">${t(template.role)}</option>`).join('')}</select>
    <label>${t('Name')}</label><input id="n-name" placeholder="${t('Writer')}">
    <label>${t('Sidebar group')}</label><select id="n-group">${groupOptions('')}</select>
    <label>${t('Role')}</label><input id="n-role" placeholder="${t('Content lead')}">
    <label>${t('What it is for (other Bots read this to hand it work)')}</label><input id="n-description" placeholder="${t('Writes posts, emails and launch copy')}">
    <label>${t('Instructions')}</label><textarea id="n-instructions" rows="5" placeholder="${t('How it works: tone, sources, what to check')}"></textarea>
    <label>${t('Model (optional)')}</label><input id="n-model" list="provider-models" placeholder="${escape(view.team.provider?.defaultModel || t('default'))}">${modelList()}
    <div class="form-actions"><button class="ghost" data-close>${t('Cancel')}</button><button class="primary" id="n-create">${t('Create')}</button></div>`)
  $('#n-name').focus()
  $('#n-template').onchange = () => {
    const template = view.team.templates.find(template => template.id === $('#n-template').value)
    if (!template) return
    for (const key of ['name', 'role', 'description', 'instructions']) $(`#n-${key}`).value = (LANG === 'zh' && template.zh?.[key]) || template[key]
  }
  $('#n-create').onclick = attempt(async () => {
    const bot = await call('bot.create', { name: $('#n-name').value, role: $('#n-role').value, instructions: $('#n-instructions').value, description: $('#n-description').value, model: $('#n-model').value })
    if ($('#n-group').value) await call('sidebar.move', { conversation: `bot:${bot.id}`, group: $('#n-group').value })
    closeModal()
    await refresh()
    open(`bot:${bot.id}`)
  })
}

function newRoomModal() {
  modal(`<h3>${t('New group chat')}</h3>
    <label>${t('Name')}</label><input id="g-name" placeholder="${t('Launch plan')}">
    <label>${t('Sidebar group')}</label><select id="g-group">${groupOptions('')}</select>
    <label>${t('Notice: what the group is for')}</label><input id="g-notice">
    <label>${t('Members (the first one is the admin)')}</label><div class="checks">${bots().map(bot => `<label><input type="checkbox" value="${escape(bot.id)}"> ${escape(bot.name)}</label>`).join('')}</div>
    <div class="form-actions"><button class="ghost" data-close>${t('Cancel')}</button><button class="primary" id="g-create">${t('Create')}</button></div>`)
  $('#g-name').focus()
  $('#g-create').onclick = attempt(async () => {
    const members = [...document.querySelectorAll('#modal .checks input:checked')].map(el => el.value)
    const room = await call('room.create', { name: $('#g-name').value, notice: $('#g-notice').value, members })
    if ($('#g-group').value) await call('sidebar.move', { conversation: `room:${room.id}`, group: $('#g-group').value })
    closeModal()
    await refresh()
    open(`room:${room.id}`)
  })
}

// ---------------------------------------------------------------------------
// Notifications, when the page is in the background

function notify(title, body, conversation) {
  if (!document.hidden || typeof Notification === 'undefined' || Notification.permission !== 'granted') return
  const note = new Notification(title, { body: short(body, 140), tag: conversation })
  note.onclick = () => { window.focus(); if (conversation) open(conversation) }
}

// ---------------------------------------------------------------------------
// The feed

async function refresh() {
  const ticket = navigationTicket
  const value = await call('team.state', {}, { signal: AbortSignal.timeout(15000) })
  const savedLang = localStorage.getItem('dotbot.lang')
  if (!view.team && !['zh', 'en'].includes(savedLang) && ['zh', 'en'].includes(value.prefs?.lang)) {
    LANG = value.prefs.lang
    translateStatic(); renderThemeButton()
  }
  view.work = value.work
  delete value.work
  value.history = mergeHistoryMeta(view.team?.history, value.history)
  view.team = value
  // The server writes Bot replies and notes in the page's language.
  if (view.team.prefs?.lang !== LANG) call('settings.lang', { lang: LANG }).catch(() => {})
  const conversation = view.current
  if (conversation && !view.page && !view.conversationLoading && ticket === navigationTicket) {
    const anchor = view.newer != null ? view.items[0]?.id : undefined
    const page = await call('chat.page', { conversation, anchor })
    if (conversation === view.current && !view.page && ticket === navigationTicket) {
      if (view.newer == null) view.items = mergeMessages(view.items, page.items)
      else { view.items = mergeMessages(page.items, view.items); view.more = page.more; view.newer = page.newer }
    }
  }
  renderAll()
}

function applyPatch(team, ops) {
  const next = { ...team }
  for (const { op, path, value } of ops) {
    const [key, id] = path
    if (id === undefined) { if (op === 'remove') delete next[key]; else next[key] = value; continue }
    const list = Array.isArray(next[key]) ? next[key] : []
    const at = list.findIndex(entry => entry.id === id)
    next[key] = op === 'remove' ? list.filter(entry => entry.id !== id) : at < 0 ? [...list, value] : list.map(entry => (entry.id === id ? value : entry))
  }
  return next
}

function belongsHere(conversation) {
  if (conversation === view.current) return true
  const room = currentRoom()
  return Boolean(room && conversation.startsWith(`room:${room.id}:`))
}
const typingInPage = () => ['INPUT', 'TEXTAREA', 'SELECT'].includes(document.activeElement?.tagName) && document.activeElement.closest('#settings-content')

function onEvent(event, data) {
  if (event === 'hello') {
    const replaced=feedInstance&&feedInstance!==data.serverInstance
    if(feedAssets&&data.assetRevision&&feedAssets!==data.assetRevision)$('#reload-ui').hidden=false
    feedAssets ||= data.assetRevision
    feedInstance=data.serverInstance
    if(replaced)view.streams={}
    void refresh().then(async()=>{
      if(replaced&&desk.bot&&$('#d-view')){
        if(desk.teaching){clearDesktopRecordingListeners();desk.teaching=false;desktopRecording.queue=[];desktopRecording.token=null;toast('服务已重启，未保存的桌面示范需要重新录制')}
        const bot=desk.bot,status=await call('desktop.status',{bot})
        if(desk.bot!==bot)return
        desk.user=status.user
        await connectDesktop()
        if(status.user&&desk.bot===bot&&desk.phase==='ready')await acquireDesktopControl(bot)
      }
    }).catch(error=>toast(error.message))
    return
  }
  if (event === 'team.patch') {
    if (!view.team || view.team.revision !== data.from) { void refresh(); return }
    const previousHistory = view.team.history
    view.team = applyPatch(view.team, data.ops)
    view.team.history = mergeHistoryMeta(previousHistory, view.team.history)
    if (!view.page && view.current && !currentBot() && !currentRoom()) { view.current = null; view.conversationLoading = false; ++navigationTicket; conversationRequest?.abort() }
    renderSidebar(); renderHead()
    if (view.page === 'channels') refreshChannelCards()
    else if (view.page && !typingInPage()) renderPage({ preserveDraft: true })
    if (view.panel === 'details' && !document.activeElement?.closest('#panel')) openPanel('details')
    return
  }
  if (event === 'local-computer') {
    view.team.localComputer = data
    renderHead(); renderLocalStatus(); syncWorkDirField()
    return
  }
  if (event === 'desktop') {
    if (data.bot !== desk.bot) return
    if (data.restarted && $('#d-view') && desk.phase !== 'starting') void connectDesktop()
    // Another tab took or gave back the controls.
    if ('user' in data && !data.user && desk.user) { clearInterval(desk.renew); desk.user = false; renderDesktopControl() }
    return
  }
  if (event === 'work') {
    const before = new Set([...(view.work.approvals ?? []), ...(view.work.cards ?? [])].map(x => x.id))
    const marks = () => { const run = runOf(view.current); return `${conversationBusy()}|${run?.items ?? ''}|${run?.queuedItems ?? ''}` }
    const wasBusy = view.current && !view.page ? marks() : null
    view.work = data
    // Stop buttons under messages come and go with the work on each message.
    if (wasBusy !== null && wasBusy !== marks()) renderMessages({ keepScroll: true })
    for (const x of [...(data.approvals ?? []), ...(data.cards ?? [])]) {
      if (!before.has(x.id)) notify(t('{name} needs you', { name: botById(x.bot)?.name ?? t('A Bot') }), x.question ?? x.name ?? toolInfo({ name: x.tool, input: x.input }).label, x.conversation)
    }
    renderSidebar(); renderHead(); renderApprovals()
    if (view.page === 'dashboard' && !typingInPage()) renderPage()
    if (view.panel === 'browser' && document.activeElement?.closest('#panel') == null) {
      const bot = currentBot()
      const teaching = Boolean(bot && view.work.teaching?.includes(bot.id))
      if (bot && teaching !== Boolean(document.querySelector('.teach.on'))) browserPanel(bot)
    }
    return
  }
  if (event === 'chat.cleared') {
    conversationCache.delete(data.conversation)
    delete view.previews[data.conversation]
    for (const key of Object.keys(view.streams)) if (key.startsWith(`${data.conversation}|`)) delete view.streams[key]
    if (data.conversation === view.current && !view.page) { view.items = []; view.more = null; view.newer = null; renderMessages(); void open(data.conversation) }
    renderSidebar()
    return
  }
  if (event === 'chat.meta') {
    if (view.team) { view.team.history ??= {}; view.team.history[data.conversation] = newerMeta(view.team.history[data.conversation], data); renderSidebar() }
    return
  }
  if (event === 'chat.item') {
    rememberPreview(data.conversation, data.item)
    // An inactive cached window is revalidated on its next visit. Do not show a
    // known-stale window after an edit or a background reply.
    if (data.conversation !== view.current) {
      conversationCache.delete(data.conversation)
      if (data.conversation.startsWith('room:')) conversationCache.delete(data.conversation.split(':').slice(0, 2).join(':'))
    }
    renderSidebar()
    if (data.item.kind === 'bot') {
      delete view.streams[`${data.conversation}|${data.item.execution || data.item.bot}`]
      if (!belongsHere(data.conversation) || document.hidden) notify(botById(data.item.bot)?.name ?? data.item.who ?? t('Bot'), data.item.text, data.conversation.startsWith('room:') ? data.conversation.split(':').slice(0, 2).join(':') : data.conversation)
    }
    if (!belongsHere(data.conversation)) return
    if (view.newer != null) return
    if (currentRoom() && data.conversation !== view.current && data.item.kind !== 'tool' && data.item.kind !== 'file') return
    const at = view.items.findIndex(item => item.id === data.item.id)
    if (at >= 0) view.items[at] = data.item; else view.items.push(data.item)
    if (!view.page) renderMessages({ keepScroll: true })
    return
  }
  if (event === 'chat.stream') {
    const key = `${data.conversation}|${data.execution || data.bot}`
    if (data.phase === 'end') delete view.streams[key]
    else view.streams[key] = { conversation: data.conversation, bot: data.bot, execution: data.execution, text: (view.streams[key]?.text ?? '') + (data.text ?? '') }
    if (data.conversation === view.current && !view.page) queueMessages()
    return
  }
  if (event === 'memory' && view.panel === 'memory') openPanel('memory')
  if (event === 'routines') {
    if (view.page === 'routines' && !typingInPage()) loadPage('routines')
  }
}

let feedInstance=null,feedAssets=null
async function follow() {
  for (let failures = 0; ; failures += 1) {
    const controller=new AbortController()
    let reader,watchdog
    try {
      watchdog=setTimeout(()=>controller.abort(),65000)
      const response = await fetch('/api/bot/feed', { credentials: 'same-origin', cache: 'no-store',signal:controller.signal })
      if(response.status===401){$('#link-state').title='登录已失效，请重新登录';void goToAdminLogin();throw new Error('登录已失效')}
      if (!response.ok || !response.body) throw new Error(`HTTP ${response.status}`)
      $('#link-state').classList.add('live')
      $('#link-state').title='已连接'
      failures = 0
      reader = response.body.getReader()
      const decoder = new TextDecoder()
      let buffer = ''
      for (;;) {
        const { value, done } = await reader.read()
        if (done) break
        clearTimeout(watchdog);watchdog=setTimeout(()=>controller.abort(),65000)
        buffer += decoder.decode(value, { stream: true }).replace(/\r\n/g, '\n')
        for (let end = buffer.indexOf('\n\n'); end >= 0; end = buffer.indexOf('\n\n')) {
          const block = buffer.slice(0, end)
          buffer = buffer.slice(end + 2)
          const event = block.match(/^event: ?(.*)$/m)?.[1]
          const data = block.split('\n').filter(line => line.startsWith('data:')).map(line => line.replace(/^data: ?/, '')).join('\n')
          if (event && data) { try { onEvent(event, JSON.parse(data)) } catch (error) { console.warn('[dotbot]', error) } }
        }
      }
    } catch (error) {
      if(error.name!=='AbortError')console.warn('[dotbot] feed', error.message)
    } finally {
      clearTimeout(watchdog);controller.abort()
      try{await reader?.cancel()}catch{}
      reader?.releaseLock()
    }
    $('#link-state').classList.remove('live')
    if(!$('#link-state').title.includes('已失效'))$('#link-state').title='连接中断，正在自动重连；草稿已保留'
    await new Promise(resolve => {
      const finish=()=>{clearTimeout(timer);window.removeEventListener('online',finish);resolve()}
      const timer=setTimeout(finish,Math.min(15000,500*2**Math.min(failures,5))*(.85+Math.random()*.3))
      window.addEventListener('online',finish,{once:true})
    })
  }
}

// ---------------------------------------------------------------------------
// Wiring

const WEEKDAYS = ['Sunday', 'Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday']
function describeWhen(w) {
  if (w.kind === 'at') return t('once, {time}', { time: new Date(w.at).toLocaleString(locale(), { year: 'numeric', month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' }) })
  if (w.kind === 'daily') return t('every day at {time}', { time: w.time })
  if (w.kind === 'weekdays') return t('weekdays at {time}', { time: w.time })
  if (w.kind === 'weekly') return t('every {day} at {time}', { day: (w.days?.length ? w.days : [w.day ?? 0]).map(d => t(WEEKDAYS[d % 7])).join(LANG === 'zh' ? '、' : ', '), time: w.time })
  if (w.kind === 'monthly') return t('monthly on day {day} at {time}', { day: w.day, time: w.time })
  if (w.kind === 'yearly') return t('every year on {month}/{day} at {time}', { month: w.month, day: w.day, time: w.time })
  if (w.kind === 'every') return t('every {n} minutes', { n: w.minutes })
  if (w.kind === 'event') return `${w.source} · ${w.event || t('All events')}`
  return t('when its webhook is called')
}
// The schedule reads like Grok's: pick how often it repeats, then only the fields that
// frequency needs. "More" holds the interval, webhook and provider-event triggers.
const ROUTINE_FREQS = [['once', 'Once'], ['daily', 'Daily'], ['weekly', 'Weekly'], ['monthly', 'Monthly'], ['yearly', 'Yearly'], ['more', 'More']]
const WEEKDAY_SHORT = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat']
const scheduleDays = w => w.kind === 'weekdays' ? [1, 2, 3, 4, 5] : w.kind === 'weekly' ? (w.days?.length ? w.days : [w.day ?? 1]) : [1]
const freqOf = kind => ({ at: 'once', daily: 'daily', weekdays: 'weekly', weekly: 'weekly', monthly: 'monthly', yearly: 'yearly' })[kind] ?? 'more'
function bindRoutineForm() {
  const kind = $('#rt-kind')
  if (!kind) return
  const pad = n => String(n).padStart(2, '0')
  const days = () => $('#rt-days').value.split(',').filter(Boolean).map(Number)
  const summary = () => { try { $('#rt-summary').textContent = t('Runs {when}', { when: describeWhen(routineWhen()) }) } catch (error) { $('#rt-summary').textContent = error.message } }
  kind.onchange = () => {
    const k = kind.value, freq = freqOf(k)
    for (const b of document.querySelectorAll('[data-rt-freq]')) { const on = b.dataset.rtFreq === freq; b.classList.toggle('on', on); b.setAttribute('aria-checked', String(on)) }
    if (freq === 'more') $('#rt-more').value = k
    const picked = new Set(days())
    for (const b of document.querySelectorAll('[data-rt-day]')) { const on = picked.has(Number(b.dataset.rtDay)); b.classList.toggle('on', on); b.setAttribute('aria-pressed', String(on)) }
    $('#rt-more-label').hidden = freq !== 'more'
    $('#rt-days-label').hidden = freq !== 'weekly'
    $('#rt-month-label').hidden = k !== 'yearly'
    $('#rt-mday-label').hidden = !['monthly', 'yearly'].includes(k)
    $('#rt-date-label').hidden = k !== 'at'
    $('#rt-time-label').hidden = ['every', 'webhook', 'event'].includes(k)
    $('#rt-minutes-label').hidden = k !== 'every'
    $('#rt-zone-label').hidden = ['webhook', 'event', 'every'].includes(k)
    $('#rt-event-fields').hidden = k !== 'event'
    if (k === 'at' && !$('#rt-date').value) { const d = new Date(Date.now() + 86400000); $('#rt-date').value = `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}` }
    if (!/^\d\d:\d\d$/.test($('#rt-time').value)) $('#rt-time').value = '07:00'
    summary()
  }
  for (const b of document.querySelectorAll('[data-rt-freq]')) b.onclick = () => {
    const freq = b.dataset.rtFreq
    kind.value = freq === 'once' ? 'at' : freq === 'more' ? $('#rt-more').value : freq
    if (freq === 'weekly' && !days().length) $('#rt-days').value = '1'
    kind.onchange()
  }
  $('#rt-more').onchange = () => { kind.value = $('#rt-more').value; kind.onchange() }
  for (const b of document.querySelectorAll('[data-rt-day]')) b.onclick = () => {
    const d = Number(b.dataset.rtDay), set = new Set(days())
    set.has(d) ? set.delete(d) : set.add(d)
    $('#rt-days').value = [...set].sort().join(','); kind.onchange()
  }
  for (const b of document.querySelectorAll('[data-rt-days]')) b.onclick = () => { $('#rt-days').value = b.dataset.rtDays; kind.onchange() }
  for (const id of ['#rt-time', '#rt-date', '#rt-minutes', '#rt-month', '#rt-mday']) $(id).addEventListener('input', summary)
  const editing = (pageData.routines ?? []).find(r => r.id === routineEditing)
  if (editing?.when.kind === 'at') {
    const parts = Object.fromEntries(new Intl.DateTimeFormat('en-CA', { timeZone: editing.timezone || undefined, year: 'numeric', month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit', hourCycle: 'h23' }).formatToParts(new Date(editing.when.at)).map(p => [p.type, p.value]))
    $('#rt-date').value = `${parts.year}-${parts.month}-${parts.day}`; $('#rt-time').value = `${parts.hour}:${parts.minute}`
  }
  kind.onchange()
}
function routineWhen() {
  const kind = $('#rt-kind').value
  const time = $('#rt-time').value.trim()
  if (kind === 'event') {
    let filters
    try { filters = JSON.parse($('#rt-filters').value || '{}') } catch { throw new Error(t('Event filters must be a JSON object with string values.')) }
    if (!filters || Array.isArray(filters) || typeof filters !== 'object' || Object.entries(filters).some(([key, value]) => !key.startsWith('/') || typeof value !== 'string')) throw new Error(t('Event filters must be a JSON object with string values.'))
    return { kind, source: $('#rt-source').value, event: $('#rt-event').value.trim(), filters, signingSecret: $('#rt-signing').value.trim() }
  }
  if (kind === 'webhook') return { kind }
  if (kind === 'every') {
    const minutes = Number($('#rt-minutes').value)
    if (!(minutes >= 5)) throw new Error(t('Give a number of minutes, 5 or more.'))
    return { kind, minutes: Math.round(minutes) }
  }
  if (!/^\d\d:\d\d$/.test(time)) throw new Error(t('Pick a time.'))
  if (kind === 'at') {
    if (!$('#rt-date').value) throw new Error(t('Pick a date and time.'))
    return { kind, at: `${$('#rt-date').value}T${time}` }
  }
  if (kind === 'weekly' || kind === 'weekdays') {
    const days = [...new Set($('#rt-days').value.split(',').filter(Boolean).map(Number))].sort()
    if (!days.length) throw new Error(t('Pick at least one day.'))
    if (days.length === 7) return { kind: 'daily', time }
    if (days.join() === '1,2,3,4,5') return { kind: 'weekdays', time }
    return days.length === 1 ? { kind: 'weekly', day: days[0], time } : { kind: 'weekly', days, time }
  }
  if (kind === 'monthly') return { kind, day: Number($('#rt-mday').value), time }
  if (kind === 'yearly') return { kind, month: Number($('#rt-month').value), day: Number($('#rt-mday').value), time }
  return { kind: 'daily', time }
}
const pairs = text => Object.fromEntries(text.split('\n').map(line => line.trim()).filter(Boolean).map(line => { const at = line.indexOf('='); return [line.slice(0, at).trim(), line.slice(at + 1).trim()] }).filter(([k]) => k))

document.addEventListener('click', (event) => {
  const target = event.target.closest('button,[data-open],.shot')
  if (!target) { if (event.target.id === 'modal') closeModal(); if (event.target.id === 'settings') closeSettings(); return }
  const d = target.dataset
  if ('appRetry' in d) void initialize()
  else if ('memoryRetry' in d) openPanel('memory')
  else if ('chatRetry' in d) void open(view.current)
  else if ('pageRetry' in d) void loadPage(view.page)
  else if ('settingsClose' in d) closeSettings()
  else if (d.groupEdit !== undefined) sidebarGroupModal(d.groupEdit)
  else if (d.groupToggle !== undefined) attempt(async () => {
    const collapsed = (view.team.prefs?.sidebarCollapsed ?? []).includes(d.groupToggle)
    await call('sidebar.group.update', { name: d.groupToggle, collapsed: !collapsed }); await refresh()
  })()
  else if (d.open) { if (target.closest('summary')) event.preventDefault(); open(d.open) }
  else if (d.panelTab) openPanel(d.panelTab)
  else if (d.draft) { $('#input').value = d.draft; autosize(); $('#input').focus() }
  else if (d.detail) { const at = d.detail.indexOf(':'); openDetail(d.detail.slice(0, at), d.detail.slice(at + 1)) }
  else if (d.listPage) {
    const at = d.listPage.lastIndexOf(':'), key = d.listPage.slice(0, at)
    listState(key).page = Number(d.listPage.slice(at + 1)); updateList(key)
    document.querySelector(`[data-list-box="${key}"]`)?.scrollIntoView({ block: 'nearest' })
  }
  else if (d.page) openPage(d.page)
  else if (d.edit !== undefined) {
    const item = view.items.find(i => i.id === d.edit)
    view.editing = { id: item.id, draft: item.text }
    renderMessages({ keepScroll: true })
    const editBox = $('#edit-box'); editBox.focus(); editBox.setSelectionRange(editBox.value.length, editBox.value.length)
  } else if (d.editCancel !== undefined) { if (!view.editing?.sending) { view.editing = null; renderMessages({ keepScroll: true }) } }
  else if (d.editSend !== undefined) attempt(async () => {
    const editing = view.editing, conversation = view.current
    if (!editing || editing.sending) return
    const text = $('#edit-box').value.trim()
    if (!text) throw new Error(t('Write the message.'))
    editing.draft = text
    editing.sending = true
    renderMessages({ keepScroll: true })
    try {
      // Stop only the message being edited; anything else queued keeps running.
      if (conversation.startsWith('room:')) { if (conversationBusy(conversation) || stopRequests.has(conversation)) await stopConversation(conversation) }
      else if (itemWork(editing.id, conversation)) await stopItem(editing.id, conversation)
      await call('chat.send', { conversation, text })
      if (view.editing === editing) view.editing = null
    } finally {
      editing.sending = false
      if (view.current === conversation && !view.page) renderMessages({ keepScroll: true })
    }
  })()
  else if (d.stopRun !== undefined) attempt(() => stopConversation())()
  else if (d.stopItem) attempt(() => stopItem(d.stopItem))()
  else if (d.rateItem) attempt(async () => {
    const restoring = view.items.find(i => i.id === d.rateItem)?.retracted
    await rateReply(d.rateItem, d.rate)
    reasonFor = d.rate === 'bad' ? d.rateItem : null
    if (reasonFor) renderMessages({ keepScroll: true })
    toast(t(restoring ? 'Reply restored; the Bot sees it again' : d.rate === 'good' ? 'Noted; the Bot will keep doing this' : d.rate === 'bad' ? 'Noted; the Bot will avoid this' : 'Feedback removed'))
  })()
  else if (d.reason !== undefined && d.reasonItem) attempt(async () => { reasonFor = null; await rateReply(d.reasonItem, 'bad', { reason: d.reason }); toast(t('Thanks, the Bot will remember why')) })()
  else if (d.reasonSubmit) void submitReason(d.reasonSubmit)
  else if (d.reasonOpen) { reasonFor = d.reasonOpen; renderMessages({ keepScroll: true }) }
  else if ('reasonSkip' in d) { reasonFor = null; renderMessages({ keepScroll: true }) }
  else if (d.retract) attempt(async () => { reasonFor = null; await rateReply(d.retract, 'bad', { retract: true }); toast(t('Retracted; the Bot will no longer use this reply')) })()
  else if ('modelSettings' in d) openPage('providers')
  else if (d.openBrowser) { if (currentBot()?.id !== d.openBrowser) { view.panel = 'browser'; open(`bot:${d.openBrowser}`) } else openPanel('browser') }
  else if (d.allow) attempt(() => call('approval.answer', { id: d.allow, allow: true }))()
  else if (d.deny) attempt(() => call('approval.answer', { id: d.deny, allow: false }))()
  else if (d.card) {
    const input = document.querySelector(`[data-card-input="${CSS.escape(d.card)}"]`)
    const answer = d.answer ?? input?.value.trim() ?? ''
    if (!answer) input?.focus()
    else attempt(() => call('card.answer', { id: d.card, answer }))()
  } else if (d.cardDismiss) attempt(() => call('card.dismiss', { id: d.cardDismiss }))()
  else if ('close' in d) closeModal()
  else if (d.forget) attempt(async () => { await call('memory.forget', JSON.parse(d.forget)); openPanel('memory') })()
  else if ('addGo' in d) {
    const input = target.previousElementSibling
    attempt(async () => { await call('memory.add', { ...JSON.parse(input.dataset.add), text: input.value }); openPanel('memory') })()
  }
  else if (d.channelDiagnostics) attempt(async () => { await showChannelDiagnostics(d.channelDiagnostics) })()
  else if (d.skillValidate) attempt(async()=>{const v=await call('skill.validate',{name:d.skillValidate});modal(`<h3>完整技能包校验</h3><p>${v.passed?'结构与语法检查通过':'存在未通过项目'} · ${v.files} 个文件</p><p class="muted">${escape(v.reason)}</p><div class="stack">${v.checks.map(c=>`<div class="row-card"><strong>${escape(c.path)}</strong><p>${c.passed===true?'通过':c.passed===false?'失败':'待试运行'}：${escape(c.reason)}</p></div>`).join('')}</div><div class="form-actions"><button class="ghost" data-close>${t('Close')}</button></div>`,true)})()
  else if (d.skillCandidates) attempt(async()=>{await showSkillCandidates(d.skillCandidates)})()
  else if (d.candidatePreview) attempt(async()=>{const c=await call('skill.candidate.get',{id:d.candidatePreview});modal(`<h3>候选流程草稿</h3><p class="muted">${escape(c.name)} · ${escape(candidateStatus(c.status))}</p><pre class="candidate-draft">${escape(c.draft || c.improvement)}</pre><div class="form-actions"><button class="ghost" data-skill-candidates="${escape(c.name)}">返回候选评测</button><button class="ghost" data-close>关闭</button></div>`,true)})()
  else if (d.candidateTest) attempt(async()=>{const value=await call('skill.candidate.test',{id:d.candidateTest});closeModal();await open(`bot:${value.bot}`);toast('已开始基线与候选对照评测，正式技能在通过前保持原版本')})()
  else if (d.candidateReject) attempt(async()=>{await call('skill.candidate.reject',{id:d.candidateReject});await showSkillCandidates(d.skillName)})()
  else if (d.skillVersions) attempt(async () => { await showSkillVersions(d.skillVersions) })()
  else if (d.skillRollback) attempt(async () => { await call('skill.rollback',{name:d.skillName,version:d.skillRollback,revision:d.skillRevision}); closeModal(); await refresh(); toast('技能版本已恢复') })()
  else if (d.skillTrial) attempt(async()=>{await showSkillTrial(d.skillTrial)})()
  else if (d.skillCaseRun) attempt(async()=>{await call('skill.trial',{name:d.skillName,case:d.skillCaseRun,bot:$('#trial-bot').value});const bot=$('#trial-bot').value;closeModal();await open(`bot:${bot}`);toast('已使用当前完整技能版本运行保存的用例')})()
  else if (d.skillTrialStart) attempt(async () => { const flow=await call('skill.trial',{name:d.skillTrialStart,bot:$('#trial-bot').value,goal:$('#trial-goal').value,acceptance:$('#trial-acceptance').value}); const bot=$('#trial-bot').value; closeModal(); await open(`bot:${bot}`); toast('技能试运行已开始') })()
  else if (d.taskCancel) attempt(async () => { await call('task.cancel', { id: d.taskCancel }); toast(t('Stopped')) })()
  else if (d.routineEdit) { routineEditing = d.routineEdit; openRoutineForm() }
  else if ('routineNew' in d) { routineEditing = null; openRoutineForm() }
  else if (d.routineRun) attempt(async () => { const accepted = await call('routine.run', { id: d.routineRun }); await loadPage('routines', false); toast(t(accepted ? 'Sent' : 'This routine is already running.')) })()
  else if (d.routineToggle) attempt(async () => { await call('routine.update', { id: d.routineToggle, enabled: d.on === '1' }); await loadPage('routines', false); toast(t(d.on === '1' ? 'Routine resumed' : 'Routine paused')) })()
  else if (d.routineDelete) attempt(async () => { if (confirm(t('Delete this routine?'))) await call('routine.update', { id: d.routineDelete, delete: true }) })()
  else if (d.skillDelete) attempt(async () => { if (confirm(t('Delete the skill {name}?', { name: d.skillDelete }))) await call('skill.delete', { name: d.skillDelete }) })()
  else if (d.connTab) { connTab = d.connTab; renderPage({ preserveDraft: true }) }
  else if ('keyNew' in d) keyModal()
  else if (d.pluginTab) { pluginTab = d.pluginTab; renderPage({ preserveDraft: true }); }
  else if (d.skillKind) { skillKind = d.skillKind; resetList('skills-installed'); renderPage({ preserveDraft: true }) }
  else if (d.mcpFilter) { mcpFilter = d.mcpFilter; resetList('mcp-installed'); renderPage({ preserveDraft: true }) }
  else if (d.mcpCategory) { mcpCategory = d.mcpCategory; resetList('mcp-store'); renderPage({ preserveDraft: true }) }
  else if (d.mcpAdd) mcpAddModal(d.mcpAdd)
  else if (d.storeCategory) { storeCategory = d.storeCategory; resetList('store'); renderPage({ preserveDraft: true }) }
  else if ('storeCommand' in d) installStoreCommand()
  else if (d.storeInstall) attempt(async () => {
    const id = d.storeInstall
    if (storeInstalls.has(id)) return
    storeInstalls.add(id); renderPage({ preserveDraft: true })
    try { const name = await call('skill.install', { source: id }); await refresh(); toast(t('Installed {name}', { name })) }
    finally { storeInstalls.delete(id); if (view.page === 'skills') renderPage({ preserveDraft: true }) }
  })()
  else if (d.skillToggle) attempt(async () => {
    const skill = view.team.skills.find(s => s.name === d.skillToggle)
    await call('skill.toggle', { name: skill.name, enabled: skill.enabled === false, revision: skill.revision })
  })()
  else if (d.skillEdit) {
    if (skillSaving) return
    closeModal()
    const skill = view.team.skills.find(s => s.name === d.skillEdit)
    skillEditing = { ...skill }; pluginTab = 'manual'
    renderPage()
    $('#sk-heading').scrollIntoView({ behavior: 'smooth' })
  } else if (d.connectionEdit) {
    closeModal()
    const c = view.team.connections.find(x => x.name === d.connectionEdit)
    connectionEditing = c.name; pluginTab = 'manual'
    renderPage()
    $('#cn-name').value = c.name; $('#cn-kind').value = c.kind
    $('#cn-target').value = c.kind === 'http' ? c.url : [c.command, ...c.args].join(' ')
    $('#cn-pairs').value = pairsText(c.kind === 'http' ? c.headers : c.env)
    $('#cn-heading').scrollIntoView({ behavior: 'smooth' })
  } else if (d.connectionToggle) attempt(async () => {
    const c = view.team.connections.find(x => x.name === d.connectionToggle)
    await call('connection.save', { ...c, enabled: !c.enabled })
    toast(c.enabled ? t('Paused: Bots do not get {name} from their next turn', { name: c.name }) : t('Resumed: every Bot has {name} from its next turn', { name: c.name }))
  })()
  else if (d.connectionDelete) attempt(async () => { if (confirm(t('Remove {name}?', { name: d.connectionDelete }))) await call('connection.delete', { name: d.connectionDelete }) })()
  else if (d.connectionTest) attempt(async () => { const result = await call('connection.test', { name: d.connectionTest }); await refresh(); toast(t(result.state === 'connected' ? 'Connection test passed' : 'Connection test failed')) })()
  else if (d.accountConnect) attempt(() => connectAccount(d.accountConnect))()
  else if (d.accountApp) accountAppModal(d.accountApp)
  else if (d.accountEdit) accountModal(view.team.accounts.find(a => a.id === d.accountEdit))
  else if (d.accountAuthorize) attempt(() => openSignIn(() => call('account.authorize', { id: d.accountAuthorize })))()
  else if (d.accountTest) attempt(async () => { try { await call('account.test', { id: d.accountTest }); toast(t('Connection test passed')) } finally { await refresh() } })()
  else if (d.accountToggle) attempt(async () => { const a = view.team.accounts.find(a => a.id === d.accountToggle); await call('account.save', { ...a, enabled: !a.enabled }); await refresh() })()
  else if (d.accountRevoke || d.accountDelete) attempt(async () => {
    if (!confirm(t('Revoke this shared account? All Bots will lose its access.'))) return
    const result = await call('account.revoke', { id: d.accountRevoke || d.accountDelete, delete: Boolean(d.accountDelete) }); await refresh(); toast(result.provider)
  })()
  else if (d.eventSetup) attempt(() => eventModal(d.eventSetup))()
  else if (d.deliveryRetry || d.deliverySkip) attempt(async () => {
    const uncertain = d.uncertain === 'true' || Boolean(d.deliverySkip)
    if (uncertain && !confirm(t('I checked the platform for the current part. Continue using the selected resend or skip action.'))) return
    await call('delivery.retry', { id: d.deliveryRetry || d.deliverySkip, reviewed: uncertain, skip: Boolean(d.deliverySkip) }); await refresh(); toast(t('Queued'))
  })()
  else if (d.channelRevoke) attempt(async () => { await call('channel.allow', { id: d.channelRevoke, sender: d.sender, allow: false }); toast(t('Access removed')) })()
  else if (d.channelAllow) attempt(async () => { await call('channel.allow', { id: d.channelAllow, sender: d.sender, allow: true }); toast(t('Allowed')) })()
  else if (d.channelLogin) attempt(async () => { await call('channel.login', { id: d.channelLogin }); toast(t('Scan the code with WeChat')) })()
  else if (d.channelReconnect) attempt(async () => { await call('channel.reconnect', { id: d.channelReconnect }); toast(t('Connecting…')) })()
  else if (d.channelNew) { channelEditing = null; channelKind = d.channelNew; channelDrafts = {}; openChannelForm() }
  else if (d.channelEdit) { channelEditing = d.channelEdit; channelDrafts = {}; openChannelForm() }
  else if (d.channelToggle) attempt(async () => {
    const c = view.team.channels.find(x => x.id === d.channelToggle)
    await call('channel.save', { ...c, enabled: !c.enabled })
  })()
  else if (d.channelDelete) attempt(async () => { if (confirm(t('Remove {name}?', { name: view.team.channels.find(x => x.id === d.channelDelete)?.name }))) await call('channel.delete', { id: d.channelDelete }) })()
  else if (d.secretDelete) attempt(async () => { if (confirm(t('Delete the key {name}?', { name: d.secretDelete }))) await call('secret.delete', { name: d.secretDelete }) })()
  else if (target.id === 'rt-cancel') { routineEditing = null; closeModal() }
  else if (target.id === 'rt-create') attempt(async () => {
    const editing = routineEditing
    target.disabled = true
    try {
      await call(editing ? 'routine.update' : 'routine.create', { id: editing, title: $('#rt-title').value, prompt: $('#rt-prompt').value, target: $('#rt-target').value, when: routineWhen(), timezone: $('#rt-timezone').value.trim() || null })
      routineEditing = null; closeModal(); await loadPage('routines', false); toast(t(editing ? 'Saved' : 'Routine created'))
    } finally { target.disabled = false }
  })()
  else if (target.id === 'sk-save') attempt(saveSkill)()
  else if (target.id === 'sk-cancel' && !skillSaving) { skillEditing = null; pluginTab = 'installed'; renderPage() }
  else if (target.id === 'sk-template' && !skillSaving) {
    if (!$('#sk-text').value.trim() || confirm(t('Replace the current steps with the workflow template?'))) $('#sk-text').value = t('## Goal\nDescribe when to use this workflow.\n\n## Inputs and prerequisites\nList parameters, required tools and authorized accounts.\n\n## Steps\n1. Inspect the current state.\n2. Execute the requested work; delegate independent parts with context and acceptance criteria.\n3. Integrate collaborator results and verify the deliverable.\n\n## Checks and recovery\nRecord checkpoints. Verify external state before retrying uncertain actions.\n\n## Deliverables\nAttach the result and report evidence, limitations and remaining work.')
  }
  else if (target.id === 'cn-save') attempt(async () => {
    const kind = $('#cn-kind').value
    const [command, ...args] = $('#cn-target').value.trim().split(/\s+/)
    const settings = kind === 'http' ? { url: $('#cn-target').value.trim(), headers: pairs($('#cn-pairs').value) } : { command, args, env: pairs($('#cn-pairs').value) }
    const before = view.team.connections.find(x => x.name === connectionEditing)
    const saved = await call('connection.save', { name: $('#cn-name').value, kind, ...settings, enabled: before?.enabled ?? true })
    // A renamed connection replaces the old one rather than sitting beside it.
    if (before && saved.name !== before.name) await call('connection.delete', { name: before.name })
    connectionEditing = null; pluginTab = 'installed'
    renderPage()
    toast(before ? t('Saved') : t('Connected: every Bot has it from its next turn'))
  })()
  else if (target.id === 'ch-save') attempt(async () => {
    for (const field of $('#ch-form').querySelectorAll('[required]')) if (!field.reportValidity()) return
    const before = view.team.channels.find(x => x.id === channelEditing)
    const kind = before?.kind ?? $('#ch-kind').value
    const settings = {}
    for (const [key, , type] of CHANNEL_KINDS[kind].fields) {
      const el = $(`#ch-f-${key}`)
      settings[key] = type === 'checkbox' ? (el.checked ? '1' : '') : el.value.trim()
    }
    const allow = $('#ch-allow').value.split(/[\n,，]/).map(a => a.trim()).filter(Boolean)
    const params = { id: before?.id ?? '', kind, name: $('#ch-name').value, target: $('#ch-target').value, settings, allow, enabled: before?.enabled ?? true }
    target.disabled = true; $('#ch-error').hidden = true
    try {
      await call('channel.save', params)
      channelEditing = null; channelDrafts = {}; closeModal()
      await refresh()
      toast(before ? t('Saved') : kind === 'wechat' ? t('Saved: press Sign in and scan the code with WeChat') : t('Connecting…'))
    } catch (error) {
      if ($('#ch-error')) { $('#ch-error').textContent = t(error.message); $('#ch-error').hidden = false }
      throw error
    } finally { target.disabled = false }
  })()
  else if (target.id === 'ch-cancel') { channelEditing = null; channelDrafts = {}; closeModal() }
  else if (target.id === 'cn-cancel') { connectionEditing = null; renderPage() }
  else if (target.id === 'cn-paste-go') attempt(async () => {
    const found = parseMcpConfig($('#cn-paste').value)
    if (found.name) $('#cn-name').value = found.name.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 32)
    $('#cn-kind').value = found.kind; $('#cn-target').value = found.target; $('#cn-pairs').value = found.pairs
    toast(t('Filled in. Check the fields, then press Connect.'))
    $('#cn-name').focus()
  })()
  else if (target.id === 'sc-save') attempt(async () => {
    if (!/^\s*[A-Za-z_][A-Za-z0-9_ ]*\s*$/.test($('#sc-name').value)) throw new Error(t('A key name is English letters, digits and _, not starting with a digit, such as GITHUB_TOKEN.'))
    const name = await call('secret.set', { name: $('#sc-name').value, value: $('#sc-value').value })
    closeModal(); await refresh(); connTab = 'keys'; if (view.page === 'connections') renderPage()
    toast(t('Key saved as {name}', { name }))
  })()
  else if (target.classList.contains('shot')) modal(`<img src="${escape(target.getAttribute('src'))}" alt="${t('Screenshot')}"><div class="form-actions"><span></span><button class="ghost" data-close>${t('Close')}</button></div>`, true)
})
document.addEventListener('keydown', (event) => {
  const reasonItem = event.target?.dataset?.reasonInput
  if (reasonItem && event.key === 'Enter' && !event.isComposing) { event.preventDefault(); void submitReason(reasonItem) }
  if (reasonItem && event.key === 'Escape') { reasonFor = null; renderMessages({ keepScroll: true }) }
  const id = event.target?.dataset?.cardInput
  if (id && event.key === 'Enter') document.querySelector(`button.primary[data-card="${CSS.escape(id)}"]`)?.click()
})

// Upload drafts belong to their conversation. A failed request keeps the user's files.
const composerDrafts = new Map()
try {
  const saved=JSON.parse(sessionStorage.getItem('dotbot.upgrade-drafts')||'null')
  if(saved&&Date.now()-saved.savedAt<15*60*1000&&Array.isArray(saved.drafts))for(const [conversation,draft]of saved.drafts){if(typeof conversation==='string'&&typeof draft.text==='string'&&Array.isArray(draft.files))composerDrafts.set(conversation,{...draft,sending:false})}
  sessionStorage.removeItem('dotbot.upgrade-drafts')
}catch{}
let attached = []
const composerDraft = conversation => {
  if (!composerDrafts.has(conversation)) composerDrafts.set(conversation, { text: '', files: [], sending: false })
  return composerDrafts.get(conversation)
}
function correctionTargets() {
  if (!view.current || view.page) return []
  const tasks = conversationTasks(view.current).filter(task => ['queued', 'running'].includes(task.status))
  const targets = tasks.map(task => ({ key: `task:${task.id}`, task: task.id, status: task.status, bot: botById(task.target), objective: taskObjective(task) }))
  for (const [key, run] of Object.entries(view.work.runs ?? {})) {
    if (!run.acceptingCorrections || run.task || !(key === view.current || (view.current.startsWith('room:') && key.startsWith(`${view.current}:`)))) continue
    const bot = botById(key.split(':').at(-1))
    targets.push({ key: `run:${key}:${run.since}`, run: key, since: run.since, bot, objective: t('Current task') })
  }
  return targets
}
function defaultCorrection(targets) {
  const tasks = targets.filter(target => target.task)
  const ownTask = runOf(view.current)?.task
  if (ownTask && tasks.some(target => target.task === ownTask)) return tasks.find(target => target.task === ownTask)
  const ownRun = targets.find(target => target.run === view.current)
  // In a Bot's own chat the message belongs to that Bot: while it is idle (only its workers
  // run) it goes as a new message, and the Bot forwards amendments with correct_task.
  if (!view.current?.startsWith('room:')) return ownRun
  // An incoming queued delegation must not steal corrections from the work
  // this agent is already doing. Coordinators still default to their worker.
  if (ownRun && !conversationTasks(view.current).some(task => activeTask(task) && task.source === view.current)) return ownRun
  // A single delegated worker takes precedence over its coordinator's summary turn.
  return tasks.find(target => target.status === 'running') || tasks[0] || targets[0]
}
// A message sent while this conversation's Bot is working goes to that work; the Bot decides
// whether it amends the task or is a new request to handle afterwards.
function workingTarget() {
  return defaultCorrection(correctionTargets())
}
function stashComposer() {
  if (!view.current) return
  const draft = composerDraft(view.current); draft.text = $('#input').value; draft.files = attached
}
function restoreComposer(conversation) {
  const draft = composerDraft(conversation)
  attached = draft.files; $('#input').value = draft.text; $('#slash-menu').hidden = true; renderAttachments(); autosize()
}
function renderAttachments() {
  $('#attachments').innerHTML = attached.map((file, i) => `<span class="thumb upload-file ${file.error ? 'upload-error' : ''}">${file.preview ? `<img src="${escape(file.preview)}" alt="">` : '<span class="upload-icon">📄</span>'}<span class="upload-info"><strong>${escape(file.name)}</strong><small>${file.pending ? t('Uploading…') : file.error ? t('Upload failed') : `${(file.size / 1024).toFixed(0)} KB`}</small></span>${file.error ? `<button type="button" class="upload-retry" data-upload-retry="${i}" aria-label="${t('Retry upload')}">↻</button>` : ''}<button type="button" data-unattach="${i}" aria-label="${t('Remove')}">✕</button></span>`).join('')
  $('#attachments').hidden = attached.length === 0
  $('#send').disabled = attached.some(f => f.pending) || Boolean(view.current && composerDraft(view.current).sending)
}
async function uploadAttachment(file, conversation) {
  file.pending = true; file.error = null
  file.controller = new AbortController()
  if (view.current === conversation) renderAttachments()
  try {
    const response = await fetch(`/api/files?${new URLSearchParams({ conversation, name: file.name })}`, { method: 'POST', credentials: 'same-origin', headers: { 'content-type': 'application/octet-stream' }, body: file.original, signal: file.controller.signal })
    if (!response.ok) { const error = await response.json().catch(() => ({})); throw new Error(error.error || (response.status === 413 ? t('A file is at most 25 MB.') : `HTTP ${response.status}`)) }
    const result = await response.json(); if (!result.ok) throw new Error(result.error)
    Object.assign(file, result.file)
  } catch (error) {
    if (error.name !== 'AbortError') { file.error = error.message; if (view.current === conversation) toast(t(error.message)) }
  } finally { file.pending = false; if (view.current === conversation) renderAttachments() }
}
function attach(files) {
  if (!view.current) return
  const conversation = view.current
  for (const original of files) {
    if (attached.length >= 8) { toast(t('Attach at most 8 files per message.')); break }
    if (original.size > 25 * 1024 * 1024) { toast(t('A file is at most 25 MB.')); continue }
    if (attached.reduce((n, f) => n + f.size, 0) + original.size > 50 * 1024 * 1024) { toast(t('Attachments total at most 50 MB per message.')); continue }
    const file = { original, name: original.name || 'file', size: original.size, mime: original.type, preview: /^image\/(png|jpeg|gif|webp)$/.test(original.type) ? URL.createObjectURL(original) : null, pending: true }
    attached.push(file); composerDraft(conversation).files = attached
    void uploadAttachment(file, conversation)
  }
  renderAttachments()
}
$('#attach').onclick = () => $('#files').click()
$('#reload-ui').onclick=attempt(async()=>{
  if(view.page||!$('#modal').hidden){toast('请先保存并关闭设置或编辑窗口，再更新界面');return}
  stashComposer()
  if([...composerDrafts.values()].some(d=>d.sending||d.files.some(f=>f.pending||f.error||!f.src))){toast('请先完成附件上传和消息发送，再更新界面');return}
  const drafts=[...composerDrafts].map(([conversation,d])=>[conversation,{text:d.text,target:d.target,correction:d.correction,files:d.files.map(({name,size,mime,src,kind})=>({name,size,mime,src,kind,pending:false}))}])
  sessionStorage.setItem('dotbot.upgrade-drafts',JSON.stringify({savedAt:Date.now(),drafts}))
  location.reload()
})
$('#files').onchange = event => { attach(event.target.files); event.target.value = '' }
$('#input').addEventListener('paste', event => { const files = [...(event.clipboardData?.files ?? [])]; if (files.length) { event.preventDefault(); attach(files) } })
$('#attachments').addEventListener('click', event => {
  const retry = event.target.closest('[data-upload-retry]')
  if (retry) { void uploadAttachment(attached[Number(retry.dataset.uploadRetry)], view.current); return }
  const remove = event.target.closest('[data-unattach]')
  if (!remove) return
  const [file] = attached.splice(Number(remove.dataset.unattach), 1)
  file?.controller?.abort(); if (file?.preview) URL.revokeObjectURL(file.preview)
  renderAttachments()
})
for (const type of ['dragover', 'drop']) $('#main').addEventListener(type, event => {
  if (![...(event.dataTransfer?.types ?? [])].includes('Files') || view.page) return
  event.preventDefault(); $('#composer').classList.toggle('drag-over', type === 'dragover')
  if (type === 'drop') attach([...event.dataTransfer.files])
})
$('#main').addEventListener('dragleave', event => { if (!$('#main').contains(event.relatedTarget)) $('#composer').classList.remove('drag-over') })
window.addEventListener('pagehide', () => { for (const draft of composerDrafts.values()) for (const file of draft.files) { file.controller?.abort(); if (file.preview) URL.revokeObjectURL(file.preview) } })

// "/" at the start of the composer lists commands: chat actions run here and are never sent;
// an installed skill becomes the start of the message, so the Bot picks that workflow.
const slash = { items: [], index: 0 }
function slashCommands() {
  const chat = Boolean(currentBot() || currentRoom())
  const press = id => () => { if (!$(id).hidden) $(id).click() }
  const local = [
    ['new', 'Start a fresh conversation; memory carries over', 'fresh', chat, async () => { await call('chat.reset', { conversation: view.current }); toast(t('A fresh conversation starts here. Memory carries over.')) }],
    ['stop', 'Stop the current work', 'stop', !$('#act-stop').hidden, press('#act-stop')],
    ['search', 'Search conversation', 'search', chat, query => { $('#chat-search').hidden = false; $('#chat-search-input').value = query; $('#chat-search-input').focus(); if (query) searchConversation() }],
    ['memory', 'Memory', 'memory', !$('#act-memory').hidden, press('#act-memory')],
    ['details', 'Details', 'details', !$('#act-details').hidden, press('#act-details')],
    ['activity', 'Team activity', 'connections', !$('#act-activity').hidden, press('#act-activity')],
    ['desktop', 'Desktop & apps', 'desktop', !$('#act-desktop').hidden, press('#act-desktop')],
    ['local', 'Local terminal', 'terminal', !$('#act-local').hidden, press('#act-local')],
    ['clear', 'Clear chat history', 'clear', !$('#act-clear').hidden, press('#act-clear')],
    ['settings', 'Settings', 'general', true, () => openPage('general')],
  ].filter(c => c[3]).map(([name, label, glyph, , run]) => ({ name, label: t(label), glyph, run }))
  const skills = (view.team?.skills ?? []).filter(s => s.enabled && !local.some(c => c.name === s.name)).map(s => ({ name: s.name, label: s.description || s.title || '', glyph: 'skills', skill: true }))
  return [...local, ...skills]
}
// "@" starting a word before the caret lists the Bots that can be called here; only
// "@Name" followed by a space or punctuation calls a Bot, so picking one adds the space.
function mentionAt() {
  const el = $('#input'), before = el.value.slice(0, el.selectionStart)
  if (el.selectionStart !== el.selectionEnd) return null
  const match = /(^|[^A-Za-z0-9_.+%-])[@＠]([^\s@＠]{0,24})$/.exec(before)
  return match && { start: before.length - match[2].length - 1, query: match[2] }
}
function mentionCandidates(query) {
  const room = currentRoom(), bot = currentBot()
  if (!room && !bot) return []
  const people = room ? room.members.map(botById).filter(Boolean) : bots().filter(b => b.id !== bot.id)
  const all = [...people.map(b => ({ mention: true, name: b.name, label: b.role || '', bot: b })), ...(room ? [{ mention: true, name: 'all', label: t('Everyone in the group') }] : [])]
  const q = query.toLowerCase()
  return [...all.filter(c => c.name.toLowerCase().startsWith(q)), ...all.filter(c => q && !c.name.toLowerCase().startsWith(q) && c.name.toLowerCase().includes(q))]
}
function pickMention(i) {
  const item = slash.items[i], at = mentionAt(), el = $('#input')
  if (!item?.mention || !at) return
  const after = el.value.slice(el.selectionStart).replace(/^ /, '')
  el.value = `${el.value.slice(0, at.start)}@${item.name} ${after}`
  const caret = at.start + item.name.length + 2
  el.setSelectionRange(caret, caret)
  autosize(); stashComposer(); closeSlash()
}
function renderSlash() {
  const match = /^\/(\S*)$/.exec($('#input').value)
  const query = match?.[1].toLowerCase()
  const all = match && !view.page ? slashCommands() : []
  const mention = !match && !view.page && document.activeElement === $('#input') ? mentionAt() : null
  slash.items = mention ? mentionCandidates(mention.query) : query ? [...all.filter(c => c.name.startsWith(query)), ...all.filter(c => !c.name.startsWith(query) && (c.name.includes(query) || c.label.toLowerCase().includes(query)))] : all
  slash.index = Math.min(slash.index, Math.max(0, slash.items.length - 1))
  const menu = $('#slash-menu')
  menu.hidden = !slash.items.length
  $('#input').setAttribute('aria-expanded', String(!menu.hidden))
  if (menu.hidden) { $('#input').removeAttribute('aria-activedescendant'); return }
  menu.innerHTML = slash.items.map((c, i) => `<button type="button" role="option" id="slash-${i}" data-slash="${i}" class="${i === slash.index ? 'on' : ''}" aria-selected="${i === slash.index}" tabindex="-1">${c.mention ? (c.bot ? avatar(c.bot, 'tiny') : icon('channels')) : icon(c.glyph)}<strong>${c.mention ? '@' : '/'}${escape(c.name)}</strong><span>${escape(c.label)}</span>${c.skill ? `<em>${t('Skill')}</em>` : ''}</button>`).join('')
  $('#input').setAttribute('aria-activedescendant', `slash-${slash.index}`)
  menu.querySelector('.on')?.scrollIntoView({ block: 'nearest' })
}
function closeSlash() { slash.items = []; slash.index = 0; renderSlash() }
// Enter runs a chat action at once; a skill (or Tab) completes it so the request can follow.
function pickSlash(i, complete = false) {
  const command = slash.items[i]
  if (!command) return
  if (command.mention) { pickMention(i); return }
  if (command.skill || complete) { $('#input').value = `/${command.name} `; stashComposer(); closeSlash(); return }
  runSlash(command, '')
}
function runSlash(command, arg) {
  $('#input').value = ''; autosize(); stashComposer(); closeSlash()
  attempt(async () => command.run(arg))()
}
$('#slash-menu').addEventListener('mousedown', event => event.preventDefault())
$('#slash-menu').addEventListener('click', event => { const button = event.target.closest('[data-slash]'); if (button) pickSlash(Number(button.dataset.slash)) })
$('#input').addEventListener('blur', () => { $('#slash-menu').hidden = true })
$('#input').addEventListener('focus', renderSlash)
$('#input').addEventListener('click', renderSlash)

$('#composer').addEventListener('submit', attempt(async event => {
  event.preventDefault()
  const conversation = view.current, draft = composerDraft(conversation)
  const typedText = $('#input').value.trim()
  let text = typedText
  const typed = /^\/(\S+)(?:\s+([\s\S]*))?$/.exec(text)
  const command = typed && slashCommands().find(c => c.name === typed[1].toLowerCase())
  if (command && !command.skill) { runSlash(command, typed[2]?.trim() ?? ''); return }
  if (command) text = typed[2]?.trim() ? t('Use the {name} skill: {text}', { name: command.name, text: typed[2].trim() }) : t('Use the {name} skill', { name: command.name })
  if ((!text && attached.length === 0) || !conversation || draft.sending) return
  if (attached.some(f => f.pending)) { toast(t('Wait for uploads to finish.')); return }
  if (attached.some(f => f.error || !f.src)) { toast(t('Retry or remove failed uploads before sending.')); return }
  const files = [...attached]
  draft.sending = true; renderAttachments()
  try {
    const working = workingTarget()
    await call(working ? 'chat.correct' : 'chat.send', { conversation, text, files: files.map(f => f.src), ...(working ? { task: working.task, run: working.run, since: working.since, auto: true } : {}) })
    for (const file of files) { const at = draft.files.indexOf(file); if (at >= 0) draft.files.splice(at, 1); if (file.preview) URL.revokeObjectURL(file.preview) }
    if (draft.text.trim() === typedText) draft.text = ''
    if (view.current === conversation) {
      if ($('#input').value.trim() === typedText) $('#input').value = ''
      attached = draft.files; autosize()
      if (view.newer != null) await loadLatest()
    }
  } finally { draft.sending = false; if (view.current === conversation) { renderAttachments(); renderHead() } }
}))
const autosize = () => { const el = $('#input'); el.style.height = 'auto'; el.style.height = `${Math.min(200, el.scrollHeight)}px` }
$('#input').addEventListener('input', () => {
  autosize(); stashComposer(); slash.index = 0; renderSlash()
})
document.addEventListener('change', event => { if (event.target.id === 'learning-enabled') attempt(async () => { await call('settings.learning', { enabled: event.target.checked }); await refresh() })() })
document.addEventListener('input', event => {
  if (event.target.dataset?.listSearch) { const key = event.target.dataset.listSearch; listState(key).q = event.target.value; resetList(key); updateList(key); return }
  if (event.target.id === 'store-command') { storeCommand = event.target.value; return }
})
$('#input').addEventListener('keydown', (event) => {
  if (!$('#slash-menu').hidden && !event.isComposing) {
    const step = { ArrowDown: 1, ArrowUp: -1 }[event.key]
    if (step) { event.preventDefault(); slash.index = (slash.index + step + slash.items.length) % slash.items.length; renderSlash(); return }
    if (event.key === 'Tab' || (event.key === 'Enter' && !event.shiftKey)) { event.preventDefault(); pickSlash(slash.index, event.key === 'Tab'); return }
    if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); $('#slash-menu').hidden = true; return }
    // Space after "@part" completes the highlighted Bot; a bare "@ " stays plain text.
    if (event.key === ' ' && slash.items[slash.index]?.mention && mentionAt()?.query) { event.preventDefault(); pickMention(slash.index); return }
  }
  if (event.key === 'Enter' && !event.shiftKey && !event.isComposing) { event.preventDefault(); $('#composer').requestSubmit() }
})
document.addEventListener('click', event => {
  const menu = $('#conversation-menu')
  if (!event.target.closest('#conversation-menu') || event.target.closest('.conversation-menu-items button')) menu.open = false
})
document.addEventListener('keydown', event => { if (event.key === 'Escape') $('#conversation-menu').open = false })
document.addEventListener('keydown', event => { if (event.key === 'Enter' && event.target.id === 'store-command' && !event.isComposing) { event.preventDefault(); installStoreCommand() } })
function updateScrollAffordance() {
  const box = $('#messages')
  $('#jump-latest').hidden = Boolean(view.page) || !view.current || (view.newer == null && box.scrollHeight - box.scrollTop - box.clientHeight < 100)
}
// When the task board, approvals or attachments above/below the list appear, the list gets
// shorter; a reader who was at the latest message stays there instead of losing it.
{
  const box = $('#messages')
  // A reload snapshot may already have scrolled the list to a reading position.
  let pinned = box.scrollHeight - box.scrollTop - box.clientHeight < 40, height = box.clientHeight
  box.addEventListener('scroll', () => { if (box.clientHeight === height) pinned = box.scrollHeight - box.scrollTop - box.clientHeight < 40 }, { passive: true })
  new ResizeObserver(() => { if (box.clientHeight !== height) { height = box.clientHeight; if (pinned && !view.restoring) box.scrollTop = box.scrollHeight } }).observe(box)
}
$('#messages').addEventListener('scroll', () => { updateScrollAffordance(); scheduleReading(); if (!view.restoring && $('#messages').scrollTop < 80) void loadHistory('older') }, { passive: true })
$('#messages').addEventListener('click', event => { if (event.target.closest('[data-load-older]')) void loadHistory('older'); if (event.target.closest('[data-load-newer]')) void loadHistory('newer') })
$('#jump-latest').onclick = () => { if (view.newer != null) void loadLatest(); else { $('#messages').scrollTo({ top: $('#messages').scrollHeight, behavior: 'smooth' }); scheduleReading() } }
$('#unread-jump').onclick = loadLatest
$('#more-unread').onclick = () => { const meta = Object.values(view.team?.history ?? {}).find(m => m.unread && (m.conversation !== view.current || view.page)); if (meta) void open(meta.conversation) }
document.addEventListener('visibilitychange', () => { if (!document.hidden) scheduleReading() })
$('#new-bot').onclick = () => { $('#new-conversation').open = false; newBotModal() }
$('#new-sidebar-group').onclick = () => { $('#new-conversation').open = false; sidebarGroupModal() }
$('#new-room').onclick = () => { $('#new-conversation').open = false; newRoomModal() }
document.addEventListener('click', event => { if (!event.target.closest('#new-conversation')) $('#new-conversation').open = false })
document.addEventListener('keydown', event => { if (event.key === 'Escape') $('#new-conversation').open = false })
$('#open-settings').onclick = () => view.team && settingsModal()
$('#panel-close').onclick = () => closePanel()
// Below 1180px the panel floats over the chat: a click anywhere outside it closes it, like the sidebar's shade.
// Header buttons toggle it themselves; dialogs, and elements a click just re-rendered away, are left alone.
document.addEventListener('click', event => {
  const target = event.target
  if (!view.panel || $('#panel').hidden || innerWidth >= 1180 || !target.isConnected) return
  if (target.closest('#panel, #head, #modal, #settings, #file-preview-dialog, #toast, .desktop-view.popup, .browser-card.popup')) return
  closePanel()
})
$('#act-panel').onclick = () => {
  if (view.panel) { closePanel(); return }
  const bot = currentBot()
  const available = kind => kind === 'desktop' ? bot && view.team?.desktop : kind === 'browser' ? bot && view.team?.browser : kind === 'local' ? bot && view.team?.localComputer?.enabled : kind === 'details' || kind === 'memory' || (kind === 'activity' && bot)
  openPanel(lastPanel && available(lastPanel) ? lastPanel : bot && view.team?.desktop ? 'desktop' : bot && view.team?.browser ? 'browser' : 'details')
}
$('#toggle-sidebar').onclick = () => setSidebar(!$('#sidebar').classList.contains('open'))
$('#sidebar-shade').onclick = () => setSidebar(false)
$('#team-search').oninput = renderSidebar
$('#filter-all').onclick = () => { view.teamFilter = 'all'; renderSidebar() }
$('#filter-working').onclick = () => { view.teamFilter = 'working'; renderSidebar() }
$('#act-search').onclick = () => { $('#chat-search').hidden = !$('#chat-search').hidden; if (!$('#chat-search').hidden) $('#chat-search-input').focus(); else { $('#chat-search-input').value = ''; searchConversation(false) } }
let searchTimer
$('#chat-search-input').oninput = () => { view.searchIndex = 0; view.searchTicket++; clearTimeout(searchTimer); searchTimer = setTimeout(() => { void searchConversation() }, 180) }
$('#search-prev').onclick = () => { view.searchIndex--; searchConversation() }
$('#search-next').onclick = () => { view.searchIndex++; searchConversation() }
$('#search-close').onclick = () => { $('#chat-search').hidden = true; $('#chat-search-input').value = ''; searchConversation(false); $('#act-search').focus() }
$('#chat-search-input').onkeydown = event => { if (event.key === 'Escape') $('#search-close').click(); else if (event.key === 'Enter') { event.preventDefault(); view.searchIndex += event.shiftKey ? -1 : 1; searchConversation() } }
document.addEventListener('keydown', event => {
  const typing = event.target.closest?.('input, textarea, select, [contenteditable="true"]')
  if (event.key === '/' && !view.page && !typing && !event.ctrlKey && !event.metaKey && !event.altKey) { event.preventDefault(); if (innerWidth <= 860) setSidebar(true); $('#team-search').focus() }
  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'f' && !view.page && view.current) { event.preventDefault(); $('#chat-search').hidden = false; $('#chat-search-input').focus() }
  if (event.key === 'Escape' && !typing) setSidebar(false)
})
matchMedia('(min-width: 861px)').addEventListener('change', event => { if (event.matches) setSidebar(false) })
$('#toggle-lang').onclick = () => setLang(LANG === 'zh' ? 'en' : 'zh')
$('#toggle-theme').onclick = () => setTheme(themeNow() === 'dark' ? 'light' : 'dark')

// 'light', 'dark', or 'system' (no attribute: the stylesheet follows the system).
const themeChoice = () => document.documentElement.dataset.theme ?? 'system'
const themeNow = () => (themeChoice() !== 'system' ? themeChoice() : matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light')
function setTheme(theme) {
  if (theme === 'light' || theme === 'dark') { document.documentElement.dataset.theme = theme; localStorage.setItem('dotbot.theme', theme) }
  else { delete document.documentElement.dataset.theme; localStorage.removeItem('dotbot.theme') }
  if ($('#s-theme')) $('#s-theme').value = theme
  renderThemeButton()
}
// The button names the mode it switches to, with its sun or moon, in the text colour.
const SUN = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true"><circle cx="12" cy="12" r="4"/><path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4"/></svg>'
const MOON = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linejoin="round" aria-hidden="true"><path d="M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8z"/></svg>'
function renderThemeButton() {
  const toDark = themeNow() !== 'dark'
  $('#toggle-theme').innerHTML = `${toDark ? MOON : SUN}<span>${t(toDark ? 'Dark' : 'Light')}</span>`
  $('#toggle-theme').title = t(toDark ? 'Switch to dark mode' : 'Switch to light mode')
}
renderThemeButton()
matchMedia('(prefers-color-scheme: dark)').addEventListener('change', renderThemeButton)

// A new language: the fixed text in the page, then everything drawn from the team.
function setLang(lang) {
  const draft = view.page ? settingsDraft().filter(field => field.id !== 's-lang') : []
  LANG = lang === 'zh' ? 'zh' : 'en'
  localStorage.setItem('dotbot.lang', LANG)
  if (view.team) call('settings.lang', { lang: LANG }).catch(() => {})
  translateStatic()
  renderThemeButton()
  if (!view.team) return
  renderAll()
  if (view.page === 'general') renderGeneralSettings()
  else if (view.page) renderPage()
  restoreSettingsDraft(draft)
  if (view.panel) openPanel(view.panel)
}
$('#act-details').onclick = () => (view.panel === 'details' ? closePanel() : openPanel('details'))
$('#act-memory').onclick = () => (view.panel === 'memory' ? closePanel() : openPanel('memory'))
$('#act-clear').onclick = attempt(async () => {
  const conversation = view.current
  if (!conversation || !confirm(t('Clear all messages in this conversation? This cannot be undone. Memory, tasks and files are kept.'))) return
  await call('chat.clear', { conversation })
  toast(t('Chat history cleared'))
})
$('#act-activity').onclick = () => (view.panel === 'activity' ? closePanel() : openPanel('activity'))
$('#act-local').onclick = () => (view.panel === 'local' ? closePanel() : openPanel('local'))
$('#act-desktop').onclick = () => {
  const panel = view.team?.desktop ? 'desktop' : 'browser'
  view.panel === panel ? closePanel() : openPanel(panel)
}
$('#act-stop').onclick = attempt(() => stopConversation())

function route() {
  const wanted = decodeURIComponent(location.hash.slice(1))
  if (wanted.startsWith('page:') && PAGES[wanted.slice(5)]) { openPage(wanted.slice(5)); return true }
  const entry = [...bots(), ...rooms()].find(e => e.conversation === wanted)
  if (entry && (wanted !== view.current || view.page)) { open(wanted); return true }
  return Boolean(entry)
}
window.addEventListener('hashchange', route)

document.addEventListener('keydown', event => {
  if ($('#settings').hidden || !$('#modal').hidden) return
  if (event.key === 'Escape') { event.preventDefault(); closeSettings(); return }
  if (event.key !== 'Tab') return
  const focusable = [...$('#settings').querySelectorAll('button:not([disabled]), input:not([disabled]), textarea:not([disabled]), select:not([disabled]), a[href]')].filter(el => el.getClientRects().length)
  const first = focusable[0], last = focusable.at(-1)
  if (!first) return
  if (event.shiftKey && (document.activeElement === first || !focusable.includes(document.activeElement))) { event.preventDefault(); last.focus() }
  else if (!event.shiftKey && (document.activeElement === last || !focusable.includes(document.activeElement))) { event.preventDefault(); first.focus() }
})

// A reload repaints the last conversation at once instead of a blank page and
// skeleton; live data then replaces it in place. The snapshot is used once.
const RELOAD_SNAPSHOT = 'dotbot.reload-snapshot'
window.addEventListener('pagehide', () => {
  try {
    sessionStorage.removeItem(RELOAD_SNAPSHOT)
    if (!view.current || view.page || view.conversationLoading || view.conversationError || !view.items.length) return
    const snapshot = JSON.stringify({ at: Date.now(), conversation: view.current, items: view.items, more: view.more, newer: view.newer, ...conversationPosition(),
      lang: document.documentElement.lang, labels: Object.fromEntries(['', 'title', 'placeholder', 'aria-label'].map(attribute => {
        const data = `data-i18n${attribute ? `-${attribute}` : ''}`
        return [attribute, Object.fromEntries([...document.querySelectorAll(`#app [${data}]`)].map(el => [el.getAttribute(data), attribute ? el.getAttribute(attribute) : el.textContent]))]
      })),
      main: $('#main').className, settings: $('#open-settings').innerHTML,
      actions: $('#head .actions').innerHTML, hint: $('#composer-hint').innerHTML,
      html: { bots: $('#bots').innerHTML, messages: $('#messages').innerHTML, avatar: $('#head-avatar').innerHTML, title: $('#title').textContent, subtitle: $('#subtitle').textContent } })
    if (snapshot.length < 3_000_000) sessionStorage.setItem(RELOAD_SNAPSHOT, snapshot)
  } catch {}
})
function restoreReloadSnapshot() {
  const saved = window.dotbotReloadSnapshot
  window.dotbotReloadSnapshot = null
  if (!saved) return false
  // translateStatic() just reset labelled buttons to plain text; put back the icons the
  // page showed, keeping the elements (and their handlers) so nothing repaints.
  const template = document.createElement('template')
  template.innerHTML = saved.actions ?? ''
  for (const el of template.content.querySelectorAll('[id]')) { const live = document.getElementById(el.id); if (live && live.innerHTML !== el.innerHTML && !el.querySelector('[id]')) live.innerHTML = el.innerHTML }
  if (saved.hint) $('#composer-hint').innerHTML = saved.hint
  const placeholder = saved.labels?.placeholder?.[$('#input').dataset.i18nPlaceholder]
  if (placeholder) $('#input').placeholder = placeholder
  if (saved.settings) $('#open-settings').innerHTML = saved.settings
  conversationCache.set(saved.conversation, { items: saved.items, more: saved.more, newer: saved.newer, anchor: saved.anchor, top: saved.top })
  if (!location.hash) history.replaceState(null, '', `#${encodeURIComponent(saved.conversation)}`)
  restoreConversationScroll(saved.anchor, saved.top)
  return true
}

translateStatic()
let initializing = false, following = false
async function initialize() {
  if (initializing) return
  initializing = true
  $('#messages').setAttribute('aria-busy', 'true')
  if (!restoreReloadSnapshot()) stableHtml($('#messages'), skeletonHtml('chat'))
  try {
    await refresh()
    if (!route() && bots()[0]) await open(bots()[0].conversation)
    if (!following) { following = true; void follow() }
  } catch (error) {
    $('#messages').setAttribute('aria-busy', 'false')
    stableHtml($('#messages'), loadErrorHtml('data-app-retry'))
    toast(t(error.message))
  } finally { initializing = false; $('#bots').inert = $('#messages').inert = false }
}
void initialize()

// Task controls and evidence stay in the existing conversation/settings surfaces.
function taskControls(task,work) {
  const id=escape(task.id)
  return `${activeTask(task) ? `<button class="ghost" data-task-cancel="${id}">${icon('stop')}${t('Stop task')}</button>` : ''}`
}
function skillQuality(name) {
  const records=bots().flatMap(bot=>(bot.skillProfile??[]).filter(s=>s.name===name).flatMap(s=>s.evaluations??[]))
  const revision=(view.team.skills??[]).find(s=>s.name===name)?.revision,current=records.filter(e=>e.revision===revision)
  return current.length ? `当前版本评估：${current.filter(e=>e.passed).length} 次通过 / ${current.length} 次评估，其中 ${current.filter(e=>e.passed&&['tool-supported-review','artifact-validation'].includes(e.evidence)).length} 次附工具回执` : '当前版本尚无验收记录'
}
async function showSkillVersions(name) {
  const skill=await call('skill.get',{name}),versions=await call('skill.versions',{name})
  modal(`<h3>技能版本与回滚</h3><p>${escape(skill.title||name)}</p><p class="muted">版本号覆盖技能文本、脚本和资源。恢复完整技能包并保留启用状态；连续三次交付物实检失败会自动停用。</p><div class="skill-version-comparison"><table><thead><tr><th>版本</th><th>已运行用例</th><th>通过 / 总次数</th></tr></thead><tbody>${versions.map(v=>`<tr><td>${escape(v.revision.slice(0,12))}${v.current?'（当前）':''}</td><td>${new Set((v.runs??[]).map(r=>r.case)).size}</td><td>${(v.runs??[]).filter(r=>r.passed===true).length} / ${(v.runs??[]).length}</td></tr>`).join('')}</tbody></table><p class="muted">仅在相同用例和相同环境下比较通过率；未运行的版本不代表更好或更差。</p></div><div class="stack">${versions.map(v=>`<div class="row-card"><strong>${escape(v.description)}</strong><p class="muted">${when(v.created)} · ${escape(v.revision.slice(0,12))}${v.current?' · 当前版本':''}</p>${!v.current?`<button class="ghost" data-skill-rollback="${escape(v.revision)}" data-skill-name="${escape(name)}" data-skill-revision="${escape(skill.revision)}">恢复此版本</button>`:''}</div>`).join('')}</div><div class="form-actions"><button class="ghost" data-close>${t('Close')}</button></div>`,true)
}
async function showSkillTrial(name) {
  const suite=await call('skill.benchmarks',{name})
  modal(`<h3>技能试运行与评估</h3><p class="muted">使用所选 Bot 自己的模型和权限执行测试任务，由该 Bot 按验收条件评估。运行结束不会自动计为验收通过。</p><label>执行 Bot<select id="trial-bot">${bots().map(bot=>`<option value="${escape(bot.id)}">${escape(botName(bot))}</option>`).join('')}</select></label>${suite.cases.length?`<details><summary>已保存的评测用例（${suite.cases.length}）</summary><div class="stack">${suite.cases.map(c=>`<div class="skill-benchmark-case"><p>${escape(short(c.goal,200))}</p><small>${escape(c.acceptance)}</small><button class="ghost" data-skill-case-run="${escape(c.id)}" data-skill-name="${escape(name)}">用当前版本运行</button></div>`).join('')}</div></details>`:''}<label>测试任务<textarea id="trial-goal" rows="4" placeholder="描述输入、需要执行的工作和实际交付物"></textarea></label><label>验收条件<textarea id="trial-acceptance" rows="3" placeholder="填写可核对的检查项目和预期结果"></textarea></label><div class="form-actions"><button class="ghost" data-close>${t('Cancel')}</button><button class="primary" data-skill-trial-start="${escape(name)}">保存用例并试运行</button></div>`)
}
async function showChannelDiagnostics(id) {
  const data=await call('channel.diagnostics',{id}),ms=n=>n==null?'—':n<1000?`${n} 毫秒`:`${(n/1000).toFixed(1)} 秒`
  const names={progress_generated:'协作中，正在发送进度',progress_sending:'协作中，发送进度',progress_sent:'协作中，已发送进度',blocked:'发送受阻',receiving:'接收附件',queued:'等待执行',model_started:'正在执行',model_finished:'等待生成回复',generated:'等待投递',sending:'正在投递',sent:'已送达平台',failed:'失败',incomplete:'超过 15 分钟没有结果'}
  modal(`<h3>社交平台延迟诊断</h3><p class="muted">从本服务收到消息开始计时，不包含消息到达本服务前的平台耗时；真实收发与模拟测试需要分别验收。</p><p>已完成样本 ${data.samples} · 中位耗时 ${ms(data.p50Ms)} · P95 ${ms(data.p95Ms)}</p><div class="diagnostic-traces">${data.traces.map(trace=>`<details class="row-card"><summary>${when(trace.received)} · ${escape(names[trace.status]||trace.status)} · ${ms(trace.totalMs)}</summary><dl>${[['附件接收','attachmentMs'],['等待执行','queueMs'],['执行与协作','modelMs'],['等待投递','deliveryQueueMs'],['平台发送','sendMs']].map(([name,key])=>`<div><dt>${name}</dt><dd>${ms(trace[key])}</dd></div>`).join('')}</dl>${trace.error?`<p class="error">${escape(trace.error)}</p>`:''}<small class="muted">${escape(trace.id)}</small></details>`).join('')||'<p class="muted">暂无记录。收到下一条平台消息后会自动记录各阶段耗时。</p>'}</div><div class="form-actions"><span class="form-actions-end"><button class="ghost" data-close>${t('Close')}</button><button class="ghost" data-detail="channel:${escape(id)}">返回平台</button></span><button class="primary" data-channel-diagnostics="${escape(id)}">刷新诊断</button></div>`,true)
}


// Record only events on remote desktop canvases. Character values, clipboard
// contents, form values and window titles never leave the iframe recorder.
const desktopRecording={token:null,bot:null,queue:[],timer:null,cleanups:[],documents:new WeakSet(),sending:Promise.resolve(),lastInput:0}
function clearDesktopRecordingListeners(){clearInterval(desktopRecording.timer);desktopRecording.timer=null;desktopRecording.cleanups.splice(0).forEach(fn=>fn());desktopRecording.documents=new WeakSet()}
function attachDesktopRecording(){
  if (!desk.teaching || desktopRecording.bot!==desk.bot) return
  const attach=doc=>{
    if(!doc||desktopRecording.documents.has(doc))return
    desktopRecording.documents.add(doc)
    let start=null,lastScroll=0
    const on=(type,fn)=>{const target=doc.defaultView || doc;target.addEventListener(type,fn,true);desktopRecording.cleanups.push(()=>target.removeEventListener(type,fn,true))}
    const canvas=event=>event.target?.closest?.('canvas,video') || [...doc.querySelectorAll('canvas,video')].find(el=>{const r=el.getBoundingClientRect();return r.width>0&&r.height>0&&event.clientX>=r.left&&event.clientX<=r.right&&event.clientY>=r.top&&event.clientY<=r.bottom})
    const add=event=>{if(desk.user && desk.teaching && desktopRecording.queue.length<500)desktopRecording.queue.push(event)}
    on('pointerdown',e=>{if(canvas(e))start={x:e.clientX,y:e.clientY}})
    on('pointerup',e=>{const target=canvas(e);if(!target)return;const r=target.getBoundingClientRect();if(!r.width||!r.height)return;const drag=start&&Math.hypot(e.clientX-start.x,e.clientY-start.y)>12;add({kind:drag?'drag':'click',button:e.button===2?'right':'left',x0:start?Math.max(0,Math.min(1,(start.x-r.left)/r.width)):undefined,y0:start?Math.max(0,Math.min(1,(start.y-r.top)/r.height)):undefined,x:Math.max(0,Math.min(1,(e.clientX-r.left)/r.width)),y:Math.max(0,Math.min(1,(e.clientY-r.top)/r.height))});start=null})
    on('wheel',e=>{if(canvas(e)&&Date.now()-lastScroll>350){lastScroll=Date.now();add({kind:'scroll',down:e.deltaY>0})}})
    on('keydown',e=>{if(!doc.querySelector('canvas,video'))return;const safe=['Enter','Tab','Escape','Backspace','Delete','ArrowUp','ArrowDown','ArrowLeft','ArrowRight','Home','End','PageUp','PageDown'];if((e.ctrlKey||e.metaKey)&&['a','c','v','x','s','z','y','f','l','t','w'].includes(e.key.toLowerCase()))add({kind:'shortcut',key:`Ctrl+${e.shiftKey?'Shift+':''}${e.key.toLowerCase()}`});else if(e.altKey&&e.key==='Tab')add({kind:'shortcut',key:'Alt+Tab'});else if(safe.includes(e.key))add({kind:'key',key:e.key});else if(e.key.length===1&&Date.now()-desktopRecording.lastInput>700){desktopRecording.lastInput=Date.now();add({kind:'input'})}})
    on('beforeinput',()=>add({kind:'input'}))
    on('input',()=>add({kind:'input'}))
    on('paste',()=>add({kind:'input'}))
  }
  const scan=doc=>{attach(doc);for(const child of doc?.querySelectorAll('iframe') ?? []){try{scan(child.contentDocument)}catch{}}}
  const tick=()=>{try{scan([...document.querySelectorAll('#d-view .desktop-stage iframe')].at(-1)?.contentDocument)}catch{};void flushDesktopRecording().catch(error=>{clearDesktopRecordingListeners();toast(t(error.message))})}
  clearInterval(desktopRecording.timer);tick();desktopRecording.timer=setInterval(tick,900)
}
function flushDesktopRecording(){
  if(!desktopRecording.queue.length||!desktopRecording.token)return desktopRecording.sending
  const events=desktopRecording.queue.splice(0,100),token=desktopRecording.token,bot=desktopRecording.bot
  desktopRecording.sending=desktopRecording.sending.catch(()=>{}).then(()=>call('desktop.teach.events',{bot,token,events})).then(result=>{if($('#d-teach-count'))$('#d-teach-count').textContent=`已录制 ${result.steps} 步`}).catch(error=>{if(desktopRecording.token===token)desktopRecording.queue.unshift(...events);throw error})
  return desktopRecording.sending
}
function renderDesktopTeaching(){
  const box=$('#d-teach-controls');if(!box)return
  const active=(view.work.teaching ?? []).includes(`desktop:${desk.bot}`) || desk.teaching
  box.innerHTML=active?`<strong id="d-teach-count">${desk.teaching&&desk.user?'正在录制桌面操作':'示范已暂停，可继续录制或保存'}</strong><button class="ghost" id="d-teach-resume">继续示范</button><label>这个任务用来做什么？<input id="d-teach-goal" placeholder="例如：整理表格并导出周报"></label><div class="form-actions"><button class="ghost" id="d-teach-cancel">取消示范</button><button class="primary" id="d-teach-finish">保存为技能</button></div>`:`<button class="ghost" id="d-teach-start">开始示范</button>`
  if($('#d-teach-start'))$('#d-teach-start').onclick=attempt(async()=>{
    const bot=desk.bot;const result=await call('desktop.teach',{bot,action:'start'});await acquireDesktopControl(bot)
    desktopRecording.token=result.token;desktopRecording.bot=bot;desktopRecording.queue=[];desktopRecording.sending=Promise.resolve();desk.teaching=true;renderDesktopTeaching();attachDesktopRecording()
  })
  if($('#d-teach-resume'))$('#d-teach-resume').onclick=attempt(async()=>{const bot=desk.bot;const result=await call('desktop.teach',{bot,action:'resume'});desktopRecording.token=result.token;desktopRecording.bot=bot;desktopRecording.sending=Promise.resolve();desk.teaching=true;await acquireDesktopControl(bot);attachDesktopRecording()})
  if($('#d-teach-cancel'))$('#d-teach-cancel').onclick=attempt(async()=>{clearDesktopRecordingListeners();await call('desktop.teach',{bot:desk.bot,action:'cancel'});desktopRecording.queue=[];desktopRecording.token=null;desk.teaching=false;giveBackDesktop();renderDesktopTeaching()})
  if($('#d-teach-finish'))$('#d-teach-finish').onclick=attempt(async()=>{
    const goal=$('#d-teach-goal').value.trim();if(!goal){toast('请填写示范任务的用途');return}
    clearDesktopRecordingListeners();while(desktopRecording.queue.length)await flushDesktopRecording();await desktopRecording.sending
    await call('desktop.teach',{bot:desk.bot,action:'finish',goal});desktopRecording.token=null;desk.teaching=false;giveBackDesktop();renderDesktopTeaching();toast('已提交示范，Bot 将整理并保存技能')
  })
}

function routingEvidenceHtml(r) {
  const p=r.performance || {}
  return `<details class="routing-evidence"><summary>查看本次分工依据</summary><p>协调 Bot 根据任务与技能相关性、历史任务记录选择。</p><p>相似任务：${p.verifiedPasses || 0} 次附证据通过 / ${p.verifiedFailures || 0} 次失败；样本 ${p.samples || 0}</p><p>选择时运行 ${p.running || 0} · 排队 ${p.queued || 0} · 耗时中位数 ${p.medianDurationMs==null?'未知':`${(p.medianDurationMs/1000).toFixed(1)} 秒`}</p><small class="muted">当前模型与权限下的历史记录仅供参考。</small></details>`
}
const candidateStatus = status => ({'awaiting-tests':'待对照评测','needs-cases':'待添加用例',testing:'评测中',promoted:'已通过并启用',failed:'评测未通过',conflict:'正式版本已变更',rejected:'已弃用'}[status] || '待核对')
async function showSkillCandidates(name) {
  const [candidates,suite]=await Promise.all([call('skill.candidates',{name}),call('skill.benchmarks',{name})])
  modal(`<h3>技能候选改进</h3><p class="muted">整理员和 Bot 对导入或用户技能的改进先保存在独立候选中。全部保存用例的基线与候选均通过，并附实际检查或执行证据，才会启用。模型、权限、环境或项目输入不一致时保留正式版本。</p><p class="muted">已有只读回执可确认安全复测的用例会在空闲时自动运行；含命令或外部操作的用例需在这里启动，仍遵守 Bot 的审批设置。评测不会合并回原项目，目录隔离不等于系统沙箱。</p><div class="stack">${candidates.map(c=>`<details class="row-card candidate-card" open><summary><strong>${escape(candidateStatus(c.status))}</strong><small>${when(c.updated)}</small></summary><p>${escape(c.reason)}</p><p class="muted">旧版 ${escape(c.baseRevision.slice(0,12))} → 候选 ${escape(c.revision.slice(0,12))}</p><pre>${escape(c.improvement)}</pre>${c.kind==='bot-update'?`<button class="ghost" data-candidate-preview="${escape(c.id)}">查看完整流程草稿</button>`:''}<p>保存用例 ${c.cases.length || suite.cases.length} · 已返回 ${c.runs.length} 次对照结果</p>${c.runs.length?`<ul>${c.runs.map(r=>`<li>${r.phase==='regbase'?'基线':'候选'} · ${escape(r.case.slice(0,8))} · ${r.passed&&r.evidence?'通过并附证据':r.passed?'待补验证证据':'未通过'}</li>`).join('')}</ul>`:''}<div class="form-actions">${['awaiting-tests','needs-cases'].includes(c.status)?`<button class="primary" data-candidate-test="${escape(c.id)}">运行全部用例并对照</button>`:''}${!['promoted','rejected'].includes(c.status)?`<button class="ghost" data-candidate-reject="${escape(c.id)}" data-skill-name="${escape(name)}">弃用候选</button>`:''}</div></details>`).join('')||'<p class="muted">暂无候选改进。整理员或 Bot 改进导入、用户技能时会创建。</p>'}</div><div class="form-actions"><button class="ghost" data-skill-trial="${escape(name)}">添加或运行评测用例</button><button class="ghost" data-skill-candidates="${escape(name)}">刷新结果</button><button class="ghost" data-close>关闭</button></div>`,true)
}
