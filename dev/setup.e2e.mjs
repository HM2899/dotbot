#!/usr/bin/env node
// First-install lifecycle against real SQLite, including restart and restricted sessions.
import assert from 'node:assert/strict'
import {mkdtempSync,mkdirSync,writeFileSync,readFileSync,rmSync,statSync} from 'node:fs'
import {tmpdir} from 'node:os'
import {join} from 'node:path'
import {startServer} from './harness.mjs'

const directory=mkdtempSync(join(tmpdir(),'dotbot-setup-'))
let app
try {
  // Legacy startup tokens never become administrator credentials on upgrade.
  mkdirSync(join(directory,'team/web-auth'),{recursive:true})
  const legacy='b'.repeat(64)
  writeFileSync(join(directory,'team/web-auth/token'),legacy)
  app=await startServer({directory,keepHome:true,bootstrap:true,storage:'sqlite',extraEnv:{DOTBOT_SKILL_DEFAULTS:'0'}})
  const initial=app.initialPassword
  assert.ok(initial?.length>=32)
  const request=(path,{cookie=app.cookie,body,origin}={})=>fetch(app.base+path,{redirect:'manual',method:body?'POST':'GET',headers:{...(cookie?{cookie}:{}),...(body?{'content-type':'application/json'}:{}),...(origin?{origin}:{})},...(body?{body:JSON.stringify(body)}:{})})
  const status=async cookie=>(await request('/auth/status',{cookie})).json()
  assert.deepEqual(await status(''),{configured:true,signedIn:false,setupRequired:true,username:null})
  assert.equal((await request('/setup',{cookie:''})).headers.get('location'),'/login')
  assert.equal((await request('/')).headers.get('location'),'/setup')
  assert.equal((await request('/login')).headers.get('location'),'/setup')
  assert.equal((await request('/setup')).status,200)
  for(const path of ['/api/bot/feed','/media/missing','/desktop/missing/']) assert.equal((await request(path)).status,401)
  assert.equal((await request('/api/bot',{body:{method:'team.state'}})).status,401)
  assert.equal((await request('/auth/password',{body:{current:initial,password:'other-private-password'}})).status,401)
  const body={username:'我的管理员',password:'my-new-private-password-123',name:'小林',bio:'偏好中文，主要进行研究与开发。'}
  assert.equal((await request('/auth/setup',{cookie:'',body})).status,401)
  assert.equal((await request('/auth/setup',{cookie:'teambot='+legacy,body})).status,401)
  assert.equal((await request('/auth/setup',{body,origin:'https://attacker.example'})).status,403)
  assert.equal((await request('/auth/setup',{body:{...body,name:''}})).status,400)
  assert.equal((await request('/auth/setup',{body:{...body,username:'bad name'}})).status,400)
  assert.equal((await request('/auth/setup',{body:{...body,password:initial}})).status,400)
  const credentials=join(directory,'team/web-auth/admin.json')
  const hash=JSON.parse(readFileSync(credentials,'utf8')).password
  assert.equal(statSync(credentials).mode&0o777,0o600)
  assert.ok(!readFileSync(credentials,'utf8').includes(initial))
  const before=app.cookie
  await app.stop()
  app=await startServer({directory,keepHome:true,bootstrap:true,adminPassword:initial,storage:'sqlite',extraEnv:{DOTBOT_SKILL_DEFAULTS:'0'}})
  assert.equal(app.initialPassword,undefined)
  assert.equal(JSON.parse(readFileSync(credentials,'utf8')).password,hash)
  assert.equal((await status(before)).signedIn,true)
  assert.equal((await status(before)).setupRequired,true)
  const result=await request('/auth/setup',{body})
  assert.equal(result.status,200)
  assert.match(result.headers.get('set-cookie'),/Max-Age=0/)
  assert.equal((await status(before)).signedIn,false)
  assert.equal((await status()).signedIn,false)
  assert.equal((await status('')).setupRequired,false)
  assert.equal((await request('/auth/setup',{body})).status,409)
  assert.equal((await request('/auth/login',{cookie:'',body:{username:'admin',password:initial}})).status,401)
  const login=await request('/auth/login',{cookie:'',body:{username:body.username,password:body.password}})
  assert.equal(login.status,200)
  const owner=login.headers.getSetCookie()[0].split(';')[0]
  const team=await (await request('/api/bot',{cookie:owner,body:{method:'team.state'}})).json()
  assert.equal(team.value.prefs.profile.name,body.name)
  assert.equal(team.value.prefs.profile.bio,body.bio)
  await app.stop()
  app=await startServer({directory,keepHome:true,bootstrap:true,adminUsername:body.username,adminPassword:body.password,storage:'sqlite',extraEnv:{DOTBOT_SKILL_DEFAULTS:'0'}})
  assert.equal(app.initialPassword,undefined)
  assert.equal((await status()).setupRequired,false)
  assert.equal((await request('/setup')).headers.get('location'),'/')
  assert.equal((await app.call('team.state')).prefs.profile.name,body.name)
  console.log('ok   generated initial password; setup-only sessions; CSRF, validation and forced password replacement')
  console.log('ok   interrupted setup survives restart; completed setup preserves SQLite profile and cannot reopen; legacy tokens rejected')
} finally { await app?.stop();rmSync(directory,{recursive:true,force:true}) }
