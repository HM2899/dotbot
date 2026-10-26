import { spawn, spawnSync } from 'node:child_process'
import { once } from 'node:events'
import { mkdtempSync, rmSync, existsSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { startFakeModel } from './fake-model.mjs'

export const root = dirname(dirname(fileURLToPath(import.meta.url)))

// Call inside try/finally and await stop(), including when setup or tests fail.
export async function startServer({ browser = false, browserCommand, desktop = false, extraArgs = [], extraEnv = {}, binary = process.env.DOTBOT_TEST_BINARY || process.env.TEAMBOT_TEST_BINARY || join(root, 'target/debug/dotbot'), directory, keepHome = false, bootstrap = false, adminPassword, adminUsername = 'admin', storage = 'json' } = {}) {
  const home = directory ?? mkdtempSync(join(tmpdir(), 'dotbot-check-'))
  const log = join(home, 'claude.jsonl')
  if (!bootstrap && !existsSync(join(home,'team/web-auth/admin.json'))) {
    const setup = spawnSync(binary,['--home',join(home,'team'),'admin-password','--username',adminUsername,'--password-stdin'], {input:(adminPassword??'isolated-admin-fixture-1234')+'\n',encoding:'utf8'})
    if (setup.status !== 0) { if (!keepHome) rmSync(home,{recursive:true,force:true}); throw new Error('administrator fixture setup failed: '+setup.stderr) }
  }
  // The fake model service stands in for a provider; the native engine calls it over HTTP.
  const model = await startFakeModel({ log })
  const args = ['--storage', storage, '--home', join(home, 'team'), 'serve', '--port', '0', '--workspace', join(home, 'work')]
  if (!desktop) args.push('--no-desktop')
  if (!browser) args.push('--no-browser')
  else args.push('--browser-command', browserCommand ?? `${process.execPath} ${join(root, 'dev/fake-browser.mjs')}`)
  args.push(...extraArgs)
  const server = spawn(binary, args, { env: { ...process.env, DOTBOT_TEST_MODEL_HEADERS: '1', FAKE_BROWSER_SCREENSHOT: join(home, 'browser.png'), DOTBOT_REVIEW_DELAY_MS: '300', ...extraEnv }, stdio: ['ignore', 'pipe', 'inherit'] })
  const exited = once(server, 'exit').catch(() => {})
  let stopped = false
  const stop = async () => {
    if (stopped) return
    stopped = true
    if (server.exitCode === null && server.signalCode === null) {
      server.kill('SIGTERM')
      const timer = setTimeout(() => server.kill('SIGKILL'), 15000)
      try { await exited } finally { clearTimeout(timer) }
    }
    await model.close()
    if (!keepHome) rmSync(home, { recursive: true, force: true })
  }
  try {
    const startup = await new Promise((resolve, reject) => {
      let text = ''
      const timer = setTimeout(() => reject(new Error('dotbot startup timed out')), 15000)
      const onData = chunk => { text += chunk; const found = text.match(/open: (\S+)/); if (found) { clearTimeout(timer); resolve({address:found[1],listen:text.match(/listen: (\S+)/)?.[1],initialPassword:text.match(/初始管理员密码（仅首次显示）：(\S+)/)?.[1]}) } }
      server.stdout.on('data', onData)
      server.once('error', error => { clearTimeout(timer); reject(error) })
      server.once('exit', code => { clearTimeout(timer); reject(new Error(`dotbot exited ${code}`)) })
    })
    const base = startup.listen || new URL(startup.address).origin
    const address = base+'/'
    adminPassword ??= startup.initialPassword ?? 'isolated-admin-fixture-1234'
    const signIn = await fetch(base+'/auth/login', {method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({username:adminUsername,password:adminPassword}),redirect:'manual'})
    if (!signIn.headers.getSetCookie().length) throw new Error('fixture sign-in failed: '+signIn.status)
    const cookie = signIn.headers.getSetCookie()[0].split(';')[0]
    const call = async (method, params = {}) => {
      const response = await fetch(`${base}/api/bot`, { method: 'POST', headers: { 'content-type': 'application/json', cookie }, body: JSON.stringify({ method, params }) })
      const result = await response.json()
      if (!result.ok) throw new Error(`${method}: ${result.error.message}`)
      return result.value
    }
    // Every start points the team at this run's fake model, reusing the provider it saved before.
    // A first-install session that must still finish setup cannot do this yet; it calls useFakeModel itself.
    const useFakeModel = async () => {
      const saved = (await call('provider.list')).providers.find(p => p.name === 'Fake model')
      const fake = await call('provider.save', { id: saved?.id ?? '', kind: 'text', name: 'Fake model', api: 'anthropic', baseUrl: model.url, apiKey: 'fake-model-key', auth: 'token', models: [], defaultModel: 'fake-model' })
      await call('provider.use', { id: fake.id })
      // Pre-reply planning and auto-approval on a fast model, as a team would choose.
      await call('provider.jobs', { jobs: { planning: 'fast-model', approve: 'fast-model' } })
    }
    if (!bootstrap) await useFakeModel()
    const authorize = context => context.addCookies([{name:'teambot',value:cookie.slice('teambot='.length),url:base,httpOnly:true,sameSite:'Strict'}])
    return { home, log, server, base, address, cookie, call, stop, authorize, model, useFakeModel, initialPassword:startup.initialPassword }
  } catch (error) { await stop(); throw error }
}

export async function until(what, check, ms = 15000) {
  const end = Date.now() + ms
  for (;;) {
    const value = await check()
    if (value) return value
    if (Date.now() > end) throw new Error(`timed out: ${what}`)
    await new Promise(resolve => setTimeout(resolve, 75))
  }
}
