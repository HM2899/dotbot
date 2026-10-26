// Local OAuth, API, webhook registration and MCP fixture. No external accounts.
import { createServer } from 'node:http'
import { once } from 'node:events'
import { createHash, createHmac, randomUUID } from 'node:crypto'
export async function startAccounts() {
  const codes = new Map(), tokens = [], requests = [], hooks = [], sessions = new Set()
  let base, refreshes = 0, revocations = 0, failIdentity = false
  const server = createServer(async (request, response) => {
    try {
      const chunks = []; for await (const chunk of request) chunks.push(chunk)
      const raw = Buffer.concat(chunks).toString(), url = new URL(request.url, base)
      let body; try { body = JSON.parse(raw || '{}') } catch { body = Object.fromEntries(new URLSearchParams(raw)) }
      requests.push({ path: url.pathname, method: request.method, body })
      const answer = (value, status = 200) => { response.writeHead(status, { 'content-type': 'application/json' }); response.end(JSON.stringify(value)) }
      if (url.pathname === '/authorize') {
        const code = randomUUID(); codes.set(code, Object.fromEntries(url.searchParams))
        const redirect = new URL(url.searchParams.get('redirect_uri')); redirect.searchParams.set('state', url.searchParams.get('state')); redirect.searchParams.set('code', code)
        response.writeHead(302, { location: redirect.toString() }); response.end(); return
      }
      if (url.pathname === '/token') {
        if (body.grant_type === 'authorization_code') {
          const pending = codes.get(body.code); codes.delete(body.code)
          if (!pending || pending.redirect_uri !== body.redirect_uri || createHash('sha256').update(body.code_verifier).digest('base64url') !== pending.code_challenge) { answer({ error: 'invalid_grant' }, 400); return }
        } else if (body.grant_type === 'refresh_token') { if (body.refresh_token !== 'fixture-refresh-secret') { answer({ error: 'invalid_grant' }, 400); return }; refreshes++ }
        const access_token = `fixture-access-secret-${tokens.length}`; tokens.push(access_token)
        answer({ access_token, refresh_token: 'fixture-refresh-secret', expires_in: body.grant_type === 'refresh_token' ? 3600 : 1, scope: 'read:user repo admin:repo_hook' }); return
      }
      if (url.pathname === '/revoke') { revocations++; answer({ ok: true }); return }
      if (url.pathname === '/mcp') {
        if (request.headers.authorization !== 'Bearer fixture-mcp-private') { answer({ error: 'unauthorized' }, 401); return }
        if (request.method === 'DELETE') { sessions.delete(request.headers['mcp-session-id']); answer({ ok: true }); return }
        if (!body.id) { response.writeHead(202); response.end(); return }
        if (body.method === 'initialize') {
          sessions.add('connection-test-session'); response.setHeader('mcp-session-id', 'connection-test-session')
          answer({ jsonrpc: '2.0', id: body.id, result: { protocolVersion: '2025-03-26', serverInfo: { name: 'fixture', version: '1' }, capabilities: { tools: {} } } }); return
        }
        if (body.method === 'tools/list' && sessions.has(request.headers['mcp-session-id'])) {
          response.writeHead(200, { 'content-type': 'text/event-stream' }); response.end(`event: message\ndata: ${JSON.stringify({ jsonrpc: '2.0', id: body.id, result: { tools: [{ name: 'fixture_read', inputSchema: { type: 'object' } }] } })}\n\n`); return
        }
        answer({ error: 'bad session' }, 400); return
      }
      if (url.pathname.startsWith('/api/')) {
        if (!tokens.some(t => request.headers.authorization === `Bearer ${t}`) && !request.headers.authorization?.startsWith('Basic ')) { answer({ error: 'unauthorized' }, 401); return }
        if (url.pathname === '/api/user') { answer(failIdentity ? { error: 'missing scope' } : { login: 'fixture-user', echoedToken: tokens.at(-1) }, failIdentity ? 403 : 200); return }
        if (url.pathname === '/api/redirect') { response.writeHead(302, { location: 'http://example.invalid/collect-token' }); response.end(); return }
        if (url.pathname === '/api/repos/owner/repo/hooks') {
          if (request.method === 'GET') answer(hooks)
          else { const hook = { ...body, id: 42 }; hooks.push(hook); answer(hook, 201) }
          return
        }
        if (url.pathname === '/api/repos/owner/repo/hooks/42' && request.method === 'PATCH') { Object.assign(hooks[0], body); answer(hooks[0]); return }
        if (url.pathname === '/api/repos/owner/repo/hooks/42' && request.method === 'GET') { answer(hooks[0]); return }
        if (url.pathname === '/api/repos/owner/repo/hooks/42' && request.method === 'DELETE') { hooks.splice(0); answer({ ok: true }); return }
        if (url.pathname === '/api/repos/owner/repo/hooks/42/tests') {
          const hook = hooks[0], payload = JSON.stringify({ zen: 'fixture ping' })
          const delivery = await fetch(hook.config.url, { method: 'POST', headers: { 'content-type': 'application/json', 'x-github-event': 'ping', 'x-github-delivery': randomUUID(), 'x-hub-signature-256': `sha256=${createHmac('sha256', hook.config.secret).update(payload).digest('hex')}` }, body: payload })
          answer({ delivered: delivery.status }, 202); return
        }
        if (url.pathname.includes('/applications/') && request.method === 'DELETE') { revocations++; answer({ ok: true }); return }
        answer({ ok: true, method: request.method }); return
      }
      answer({ error: 'unknown path' }, 404)
    } catch (error) { response.writeHead(500); response.end('fixture error') }
  })
  server.listen(0, '127.0.0.1'); await once(server, 'listening'); base = `http://127.0.0.1:${server.address().port}`
  return { base, requests, hooks, sessions, tokens, refreshes: () => refreshes, revocations: () => revocations,
    endpoints: { authorize: `${base}/authorize`, token: `${base}/token`, api: `${base}/api`, identity: '/user', revoke: `${base}/revoke` },
    failIdentity: value => { failIdentity = value },
    async stop() { const stopped = new Promise(resolve => server.close(resolve)); server.closeAllConnections(); await stopped }
  }
}
