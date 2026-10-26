// A local Telegram Bot API fixture. No external accounts or network requests.
import { createServer } from 'node:http'
import { once } from 'node:events'
import { createCipheriv, createDecipheriv, randomBytes, createHash } from 'node:crypto'
export async function startTelegram() {
  const updates = [], sent = [], documents = [], offsets = [], timers = new Set(), files = new Map(), downloaded = []
  let next = 1, failure = '', pollFailure = ''
  const server = createServer(async (request, response) => {
    const chunks = []; for await (const chunk of request) chunks.push(chunk)
    const bytes = Buffer.concat(chunks), raw = bytes.toString()
    let body = {}; try { body = JSON.parse(raw || '{}') } catch {}
    const url = new URL(request.url, 'http://localhost')
    const method = url.pathname.split('/').at(-1)
    const answer = value => { if (!response.destroyed) { response.setHeader('content-type', 'application/json'); response.end(JSON.stringify(value)) } }
    if (url.pathname.startsWith('/file/')) {
      const id = url.pathname.split('/').at(-1), file = files.get(id)
      downloaded.push(id)
      if (file) { response.end(file.bytes); return }
      response.statusCode = 404; response.end(); return
    }
    if (method === 'getFile') {
      const id = url.searchParams.get('file_id'), file = files.get(id)
      answer(file ? { ok: true, result: { file_path: `fixture/${id}`, file_size: file.bytes.length } } : { ok: false, description: 'Missing file' })
    }
    else if (method === 'sendDocument') {
      const form = parseMultipart(bytes, request.headers['content-type'])
      documents.push(form)
      answer(failure ? { ok: false, description: failure } : { ok: true, result: { message_id: sent.length + documents.length } })
    }
    else if (method === 'getMe') answer({ ok: true, result: { id: 900, username: 'TestBot' } })
    else if (method === 'getUpdates') {
      offsets.push(body.offset || 0)
      const timer = setTimeout(() => { timers.delete(timer); answer(pollFailure ? { ok: false, description: pollFailure } : { ok: true, result: updates.filter(update => update.update_id >= (body.offset || 0)) }) }, 70)
      timers.add(timer)
    } else if (method === 'sendMessage') {
      sent.push(body)
      answer(failure ? { ok: false, description: failure } : { ok: true, result: { message_id: sent.length } })
    } else { response.statusCode = 404; answer({ ok: false, description: 'Unknown fixture endpoint' }) }
  })
  server.listen(0, '127.0.0.1'); await once(server, 'listening')
  return {
    base: `http://127.0.0.1:${server.address().port}`, updates, sent, documents, offsets, files, downloaded,
    failSend: text => { failure = text }, failPoll: text => { pollFailure = text },
    push(text, { sender = 42, group = false, updateId, ...extra } = {}) {
      const update = { update_id: updateId ?? next++, message: { message_id: next * 10, from: { id: sender, first_name: `Sender ${sender}` }, chat: { id: group ? -100 : sender, type: group ? 'group' : 'private' }, text, ...extra } }
      updates.push(update); return update
    },
    async stop() { for (const timer of timers) clearTimeout(timer); const stopped = new Promise(resolve => server.close(resolve)); server.closeAllConnections(); await stopped },
  }
}

export async function startWeChat() {
  const timers = new Set()
  let state = 'wait', base = '', code = 0, polls = 0, hangNextPoll = false, failNextPoll = false, hungPolls = 0
  const updatePolls = []
  const messages = [], sent = [], uploaded = [], blobs = new Map(), downloads = [], tickets = new Map()
  const server = createServer(async (request, response) => {
    const chunks = []; for await (const chunk of request) chunks.push(chunk)
    const raw = Buffer.concat(chunks)
    let body = {}; try { body = JSON.parse(raw.toString() || '{}') } catch {}
    const url = new URL(request.url, 'http://localhost')
    const answer = value => { if (!response.destroyed) { response.setHeader('content-type', 'application/json'); response.end(JSON.stringify(value)) } }
    const delayed = value => { const timer = setTimeout(() => { timers.delete(timer); answer(value) }, 70); timers.add(timer) }
    if (url.pathname === '/cdn/upload') {
      const id = url.searchParams.get('id'), ticket = tickets.get(id)
      const cipher = createDecipheriv('aes-128-ecb', Buffer.from(ticket.aeskey, 'hex'), null)
      const bytes = Buffer.concat([cipher.update(raw), cipher.final()])
      uploaded.push({ ...ticket, bytes, digest: createHash('md5').update(bytes).digest('hex') })
      response.setHeader('x-encrypted-param', id); response.end(); return
    }
    if (url.pathname === '/cdn/download') {
      const id = url.searchParams.get('encrypted_query_param'); downloads.push(id)
      const bytes = blobs.get(id); if (!bytes) { response.statusCode = 404; response.end(); return }
      response.end(bytes); return
    }
    if (url.pathname.endsWith('/getuploadurl')) {
      tickets.set(body.filekey, body)
      answer({ ret: 0, upload_full_url: `${base}/cdn/upload?id=${body.filekey}` })
    }
    else if (url.pathname.endsWith('/sendmessage')) { sent.push(body.msg); answer({ ret: 0 }) }
    else if (url.pathname.endsWith('/get_bot_qrcode')) { code++; answer({ qrcode: `login-${code}`, qrcode_img_content: `https://example.invalid/login/${code}` }) }
    else if (url.pathname.endsWith('/get_qrcode_status')) { polls++; delayed({ status: state, bot_token: 'private-wechat-fixture-token', ilink_user_id: 'wx-user-42', baseurl: base }) }
    else if (url.pathname.endsWith('/getupdates')) {
      updatePolls.push({ at: Date.now(), cursor: body.get_updates_buf || '' })
      if (hangNextPoll) { hangNextPoll = false; hungPolls++; return }
      if (failNextPoll) { failNextPoll = false; response.statusCode = 503; answer({ ret: 503 }); return }
      delayed({ ret: 0, longpolling_timeout_ms: 1000, get_updates_buf: `wx-${messages.length}`, msgs: messages.slice(Number(String(body.get_updates_buf || '').replace('wx-', '')) || 0) })
    }
    else { response.statusCode = 404; answer({ ret: 404 }) }
  })
  server.listen(0, '127.0.0.1'); await once(server, 'listening'); base = `http://127.0.0.1:${server.address().port}`
  return { base, sent, uploaded, downloads, updatePolls,
    hangNextPoll: () => { hangNextPoll = true }, hungPolls: () => hungPolls,
    failNextPoll: () => { failNextPoll = true },
    push(text, { files = [], sender = 'wx-user-42' } = {}) {
      const item_list = text ? [{ type: 1, text_item: { text } }] : []
      for (const file of files) {
        const key = randomBytes(16), id = `blob-${blobs.size}`, cipher = createCipheriv('aes-128-ecb', key, null)
        blobs.set(id, Buffer.concat([cipher.update(file.bytes), cipher.final()]))
        item_list.push({ type: file.image ? 2 : 4, [file.image ? 'image_item' : 'file_item']: { file_name: file.name, len: String(file.bytes.length), media: { encrypt_query_param: id, aes_key: Buffer.from(key.toString('hex')).toString('base64') } } })
      }
      const message = { message_type: 1, message_id: `wx-message-${messages.length}`, from_user_id: sender, context_token: 'fixture-context', item_list }
      messages.push(message); return message
    },
    setStatus: value => { state = value }, polls: () => polls, async stop() { for (const timer of timers) clearTimeout(timer); const closed = new Promise(resolve => server.close(resolve)); server.closeAllConnections(); await closed } }
}

// Parse only the fields our fixture receives; preserve binary bytes exactly.
function parseMultipart(bytes, contentType) {
  const boundary = /boundary=(?:"([^"]+)"|([^;]+))/.exec(contentType || '')
  if (!boundary) throw new Error('Missing multipart boundary')
  const marker = Buffer.from(`--${boundary[1] || boundary[2]}`), result = { fields: {}, files: [] }
  let at = bytes.indexOf(marker)
  for (;;) {
    const start = bytes.indexOf(Buffer.from('\r\n\r\n'), at), end = bytes.indexOf(marker, at + marker.length)
    if (start < 0 || end < 0 || start > end) break
    const header = bytes.subarray(at, start).toString(), data = bytes.subarray(start + 4, end - 2)
    const name = /name="([^"]+)"/.exec(header)?.[1], filename = /filename="([^"]*)"/.exec(header)?.[1]
    if (filename !== undefined) result.files.push({ name, filename, bytes: data })
    else if (name) result.fields[name] = data.toString()
    at = end
  }
  return result
}
