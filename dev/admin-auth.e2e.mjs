#!/usr/bin/env node
// Isolated administrator/authentication regression; never prints credentials.
import assert from 'node:assert/strict'
import {mkdtempSync,rmSync,readFileSync,statSync,mkdirSync,writeFileSync} from 'node:fs'
import {spawnSync} from 'node:child_process'
import {tmpdir} from 'node:os'
import {join} from 'node:path'
import {startServer,root,until} from './harness.mjs'

const directory=mkdtempSync(join(tmpdir(),'dotbot-admin-auth-'))
const password='fixture-password-before-123', replacement='fixture-password-after-456'
let app
try {
  const token='a'.repeat(64)
  mkdirSync(join(directory,'team/web-auth'),{recursive:true})
  writeFileSync(join(directory,'team/web-auth/token'),token)
  app=await startServer({directory,keepHome:true,adminPassword:password,extraEnv:{DOTBOT_SKILL_DEFAULTS:'0'}})
  const request=(path,{cookie=app.cookie,origin,body,method='GET',extra={}}={})=>fetch(app.base+path,{method,redirect:'manual',headers:{...(cookie?{cookie}:{}),...(origin?{origin}:{}),...(body?{'content-type':'application/json'}:{}),...extra},...(body?{body:JSON.stringify(body)}:{})})
  const login=(user= 'admin',pass=password)=>request('/auth/login',{cookie:'',method:'POST',body:{username:user,password:pass}})
  const unauth=await request('/',{cookie:''});assert.equal(unauth.status,303);assert.equal(unauth.headers.get('location'),'/login')
  assert.equal((await (await request('/auth/status',{cookie:''})).json()).username,null)
  assert.equal((await (await request('/auth/status')).json()).username,'admin')
  const page=await request('/login',{cookie:''});assert.equal(page.status,200);assert.match(page.headers.get('content-security-policy'),/frame-ancestors 'none'/);assert.equal(page.headers.get('x-frame-options'),'DENY')
  for (const path of ['/api/bot/feed','/media/missing','/desktop/missing/']) assert.equal((await request(path,{cookie:''})).status,401)
  assert.equal((await request('/api/bot',{cookie:'',method:'POST',body:{method:'team.state'}})).status,401)
  for(const path of ['/register','/auth/register','/auth/reset']) assert.equal((await request(path,{cookie:'',method:'POST'})).status,404)
  assert.equal((await login('other')).status,401);assert.equal((await login('admin','wrong')).status,401)
  const denied=await request('/auth/login',{cookie:'',method:'POST',origin:'https://attacker.example',body:{username:'admin',password}});assert.equal(denied.status,403)
  assert.equal((await request('/auth/logout',{method:'POST',origin:'https://attacker.example'})).status,403)
  assert.equal((await request('/api/bot',{method:'POST',body:{method:'team.state'},extra:{'sec-fetch-site':'cross-site'}})).status,403)
  assert.equal((await request('/auth/login',{cookie:'',method:'POST',body:{username:'admin',password:'x'.repeat(5000)}})).status,413)
  const legacy=await request('/?token='+token,{cookie:''});assert.equal(legacy.status,303);assert.equal(legacy.headers.get('set-cookie'),null)
  assert.equal((await request('/api/bot',{cookie:'teambot='+token,method:'POST',body:{method:'team.state'}})).status,401)
  assert.equal((await request('/api/bot',{cookie:'',extra:{authorization:'Bearer '+token},method:'POST',body:{method:'team.state'}})).status,401)
  assert.equal(readFileSync(join(directory,'team/web-auth/token'),'utf8'),token)
  assert.equal(statSync(join(directory,'team/web-auth/admin.json')).mode&0o777,0o600)
  assert.ok(!readFileSync(join(directory,'team/web-auth/admin.json'),'utf8').includes(password))
  console.log('ok   administrator only; protected APIs, CSRF, body limits, private hash file and disabled legacy links')

  const oldCookie=app.cookie, base=app.base
  let restarts=0; app.server.stdout.on('data',chunk=>{restarts+=(chunk.toString().match(/open: /g)||[]).length})
  app.server.kill('SIGUSR2')
  await until('administrator session survives exec',async()=>{try{return restarts&&(await request('/')).status===200}catch{return false}},30000)
  assert.equal(app.base,base)
  await app.stop()
  app=await startServer({directory,keepHome:true,adminPassword:password,extraEnv:{DOTBOT_SKILL_DEFAULTS:'0'}})
  assert.equal((await request('/',{cookie:oldCookie})).status,200)
  const feed=await request('/api/bot/feed',{cookie:oldCookie}),reader=feed.body.getReader()
  await reader.read()
  assert.equal((await request('/auth/logout',{cookie:oldCookie,method:'POST'})).status,200)
  assert.equal((await request('/api/bot/feed',{cookie:oldCookie})).status,401)
  await Promise.race([(async()=>{for(;;){if((await reader.read()).done)break}})(),new Promise((_,reject)=>{const timer=setTimeout(()=>reject(new Error('revoked feed remains open')),4000);timer.unref()})]);reader.releaseLock()
  console.log('ok   persistent sessions survive normal restart and exec; logout revokes session and live feed')
  const current=app.cookie
  assert.equal((await request('/auth/password',{method:'POST',body:{current:password,username:'bad name'}})).status,400)
  assert.equal((await request('/auth/password',{method:'POST',body:{current:'wrong',username:'team.owner'}})).status,401)
  const change=await request('/auth/password',{method:'POST',body:{current:password,username:'team.owner',password:replacement}});assert.equal(change.status,200)
  assert.equal((await request('/api/bot/feed',{cookie:current})).status,401)
  assert.equal((await login('admin',password)).status,401)
  assert.equal((await login('admin',replacement)).status,401)
  const newLogin=await login('team.owner',replacement);assert.equal(newLogin.status,200)
  const newCookie=newLogin.headers.getSetCookie()[0].split(';')[0]
  assert.match(newLogin.headers.get('set-cookie'),/HttpOnly; SameSite=Strict; Max-Age=604800/)
  assert.equal((await request('/',{cookie:newCookie})).status,200)
  assert.equal((await (await request('/auth/status',{cookie:newCookie})).json()).username,'team.owner')
  await app.stop()
  app=await startServer({directory,keepHome:true,adminPassword:replacement,adminUsername:'team.owner',extraArgs:['--rotate-login'],extraEnv:{DOTBOT_SKILL_DEFAULTS:'0'}})
  assert.equal((await request('/api/bot/feed',{cookie:newCookie})).status,401)
  for(let i=0;i<21;i++)await login('admin','wrong')
  const limited=await login('team.owner',replacement);assert.equal(limited.status,429);assert.equal(limited.headers.get('retry-after'),'60')
  console.log('ok   password change and explicit rotation revoke all devices; bounded login rate')
  await app.stop()
  const binary=process.env.DOTBOT_TEST_BINARY||join(root,'target/debug/dotbot')
  mkdirSync(join(directory,'damaged-team/web-auth'),{recursive:true})
  writeFileSync(join(directory,'damaged-team/web-auth/admin.json'),'damaged')
  const damaged=spawnSync(binary,['--home',join(directory,'damaged-team'),'serve','--no-desktop','--no-browser'],{encoding:'utf8',timeout:15000})
  assert.notEqual(damaged.status,0);assert.match(damaged.stderr,/损坏/)
  assert.equal(readFileSync(join(directory,'damaged-team/web-auth/admin.json'),'utf8'),'damaged')
  const http=spawnSync(binary,['--home',join(directory,'team'),'serve','--no-desktop','--no-browser','--public-url','http://team.example.com'],{encoding:'utf8',timeout:15000})
  assert.notEqual(http.status,0);assert.match(http.stderr,/HTTPS/)
  console.log('ok   damaged administrator files never reset automatically; public origin requires HTTPS')
} finally { await app?.stop();rmSync(directory,{recursive:true,force:true}) }
