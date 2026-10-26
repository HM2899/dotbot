#!/usr/bin/env node
import assert from 'node:assert/strict'
import { mkdirSync, writeFileSync, readFileSync } from 'node:fs'
import { join } from 'node:path'
import { startServer, until } from './harness.mjs'
let app
try {
  app = await startServer()
  const { call, base, cookie, home } = app
  const chief = (await call('team.state')).bots[0], conversation = chief.conversation
  const items = async (c = conversation) => (await call('chat.page', { conversation: c })).items
  const idle = c => until('idle', async () => !(await call('team.state')).work.runs[c])
  async function upload(name, bytes, c = conversation, auth = cookie) {
    const response = await fetch(`${base}/api/files?${new URLSearchParams({ conversation: c, name })}`, { method: 'POST', headers: { cookie: auth, 'content-type': 'application/octet-stream' }, body: bytes })
    return { response, result: response.headers.get('content-type')?.includes('json') ? await response.json() : null }
  }
  const marker = '附件内容验证-9cfe', textBytes = Buffer.from(marker + '\nsecond line')
  const text = (await upload('../报告.txt', textBytes)).result.file
  assert.equal(text.name, '报告.txt')
  const image = (await upload('image.png', Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==', 'base64'))).result.file
  const pdfBytes = Buffer.from('%PDF-1.4\n1 0 obj << /Type /Catalog >> endobj\n%%EOF')
  const pdf = (await upload('report.pdf', pdfBytes)).result.file
  const zipBytes = Buffer.from('504b0506000000000000000000000000000000000000', 'hex')
  const zip = (await upload('bundle.zip', zipBytes)).result.file
  assert.ok(!(await items()).some(i => i.kind === 'upload'))
  assert.equal((await upload('a.txt', textBytes, conversation, '')).response.status, 401)
  assert.equal((await upload('a.txt', textBytes, 'bot:missing')).response.status, 400)
  assert.equal((await upload('fake.png', textBytes)).response.status, 400)
  assert.equal((await upload('huge.bin', Buffer.alloc(25 * 1024 * 1024 + 1))).response.status, 413)
  await call('chat.send', { conversation, text: `REPORT_ATTACHMENT ${marker}`, files: [text.src, image.src, pdf.src, zip.src] })
  await until('native image, PDF and text content', async () => (await items()).some(i => i.kind === 'bot' && i.text === 'Attachments: images=1 pdf=1; text=true'))
  const user = (await items()).find(i => i.kind === 'user' && i.files?.length === 4)
  assert.equal(user.files[0].name, '报告.txt')
  assert.deepEqual(Buffer.from(await (await fetch(`${base}/${zip.src}`, { headers: { cookie } })).arrayBuffer()), zipBytes)
  const html = (await upload('example.html', Buffer.from('<script>parent.alert(1)</script>'))).result.file
  const htmlResponse = await fetch(`${base}/${html.src}`, { headers: { cookie } })
  assert.match(htmlResponse.headers.get('content-disposition'), /^attachment/)
  assert.match(htmlResponse.headers.get('content-security-policy'), /sandbox/)
  assert.equal(htmlResponse.headers.get('x-content-type-options'), 'nosniff')
  console.log('ok   generic uploads preserve bytes and names, reach the model, enforce limits and isolate active content')

  const specialist = await call('bot.create', { name: 'Attachment reader', role: 'Reader' })
  await assert.rejects(call('chat.send', { conversation: `bot:${specialist.id}`, text: 'read private file', files: [pdf.src] }), /not attached/)
  await assert.rejects(call('chat.send', { conversation, text: 'traversal', files: ['media/../state.json'] }), /Invalid media/)
  await idle(conversation)
  await call('chat.send', { conversation, text: `TOOL message_bot ${JSON.stringify({ bot: specialist.name, text: `REPORT_ATTACHMENT ${marker}`, files: [text.src, image.src, pdf.src] })}` })
  await until('delegated attachment contents', async () => (await items(`bot:${specialist.id}`)).some(i => i.kind === 'bot' && i.text === 'Attachments: images=1 pdf=1; text=true'))
  assert.ok((await items(`bot:${specialist.id}`)).some(i => i.kind === 'file' && i.name === '报告.txt'))
  console.log('ok   upload references stay private to a conversation until explicitly delegated; files work without Docker')

  const room = await call('room.create', { name: 'Attachment group', members: [specialist.id] })
  const roomChat = `room:${room.id}`
  const groupFile = (await upload('group.pdf', pdfBytes, roomChat)).result.file
  await call('chat.send', { conversation: roomChat, text: 'REPORT_ATTACHMENT anything', files: [groupFile.src] })
  await until('group receives a PDF', async () => (await items(roomChat)).some(i => i.kind === 'bot' && i.text.startsWith('Attachments: images=0 pdf=1')))
  for (let i = 0; i < 215; i++) await call('chat.send', { conversation: roomChat, text: `retained-room-${String(i).padStart(3, '0')}` })
  const found = await call('chat.search', { conversation: roomChat, query: 'retained-room-000' })
  assert.ok(found.total >= 1)
  const older = await call('chat.page', { conversation: roomChat, anchor: found.matches[0].id })
  assert.ok(older.items.some(i => i.text === 'retained-room-000'))
  const state = JSON.parse(readFileSync(join(home, 'team/state.json')))
  assert.ok(state.rooms[room.id].log.length <= 200)
  assert.ok(!(state.rooms[room.id].log).some(i => i.text === 'retained-room-000'))
  console.log('ok   group attachments reach members; full group history survives the rolling state log')

  const archive = await call('bot.create', { name: 'History fixture', role: 'Archivist' }), archiveChat = `bot:${archive.id}`
  const rows = Array.from({ length: 540 }, (_, i) => ({ id: `archive-${i}`, kind: 'bot', bot: archive.id, time: i + 1, text: i === 3 ? '最早的唯一检索内容' : `history message ${i}` }))
  mkdirSync(join(home, 'team/chats'), { recursive: true })
  writeFileSync(join(home, `team/chats/bot_${archive.id}.jsonl`), rows.map(r => JSON.stringify(r)).join('\n') + '\n')
  const latest = await call('chat.page', { conversation: archiveChat })
  assert.equal(latest.items.length, 200); assert.equal(latest.more, 340)
  const previous = await call('chat.page', { conversation: archiveChat, before: latest.more })
  assert.equal(previous.items.length, 200); assert.equal(previous.more, 140)
  const oldest = await call('chat.page', { conversation: archiveChat, before: previous.more })
  assert.equal(oldest.items.length, 140); assert.equal(oldest.more, null)
  const search = await call('chat.search', { conversation: archiveChat, query: '唯一检索' })
  assert.equal(search.total, 1); assert.equal(search.matches[0].id, 'archive-3')
  const around = await call('chat.page', { conversation: archiveChat, anchor: 'archive-3' })
  assert.ok(around.items.some(i => i.id === 'archive-3')); assert.ok(around.newer > 0)
  assert.equal((await call('team.state')).history[archiveChat].unread, 540)
  let meta = await call('chat.read', { conversation: archiveChat, through: 'archive-239', anchor: 'archive-210', offset: -7 })
  assert.equal(meta.unread, 300)
  meta = await call('chat.read', { conversation: archiveChat, through: 'archive-20', anchor: 'archive-20', offset: 3 })
  assert.equal(meta.unread, 300)
  await Promise.all([call('chat.read', { conversation: archiveChat, through: 'archive-500' }), call('chat.read', { conversation: archiveChat, through: 'archive-100' })])
  meta = (await call('team.state')).history[archiveChat]
  assert.equal(meta.unread, 39)
  const saved = JSON.parse(readFileSync(join(home, 'team/reading.json')))[archiveChat]
  assert.equal(saved.through, 'archive-500'); assert.equal(saved.anchor, 'archive-20')
  await assert.rejects(call('chat.read', { conversation: archiveChat, through: 'nonexistent' }), /Unknown message/)
  console.log('ok   complete paging/search, older/newer windows, durable reading positions and monotonic cross-device read state work')
} finally { await app?.stop(); console.log('ok   isolated upload/history service cleaned up') }
