#!/usr/bin/env node
// Real HTTPS reverse proxy and one owned browser/context/page, including guaranteed cleanup.
import assert from 'node:assert/strict'
import https from 'node:https'
import http from 'node:http'
import {mkdtempSync,rmSync,readFileSync,mkdirSync} from 'node:fs'
import {execFileSync} from 'node:child_process'
import {once} from 'node:events'
import {tmpdir} from 'node:os'
import {join} from 'node:path'
import {chromium} from 'playwright'
import {startServer,root} from './harness.mjs'

const directory=mkdtempSync(join(tmpdir(),'dotbot-admin-ui-'))
const password='browser-fixture-password-123',replacement='browser-updated-password-456'
const artifacts=join(root,'test-results');mkdirSync(artifacts,{recursive:true})
let app,proxy,browserServer,browser,context,page,owned=[]
const alive=pid=>{try{return readFileSync(`/proc/${pid}/stat`,'utf8').split(') ')[1][0]!=='Z'}catch{return false}}
function descendants(pid){
  const rows=execFileSync('ps',['-eo','pid=,ppid='],{encoding:'utf8'}).trim().split('\n').map(s=>s.trim().split(/\s+/).map(Number))
  const ids=new Set([pid]);let size
  do{size=ids.size;for(const [child,parent]of rows)if(ids.has(parent))ids.add(child)}while(size!==ids.size)
  return [...ids]
}
try{
  execFileSync('openssl',['req','-x509','-newkey','rsa:2048','-nodes','-days','1','-subj','/CN=localhost','-keyout',join(directory,'key.pem'),'-out',join(directory,'cert.pem')],{stdio:'ignore'})
  proxy=https.createServer({key:readFileSync(join(directory,'key.pem')),cert:readFileSync(join(directory,'cert.pem'))},(req,res)=>{
    const upstream=http.request(app.base+req.url,{method:req.method,headers:req.headers},response=>{res.writeHead(response.statusCode,response.headers);response.pipe(res)})
    upstream.on('error',()=>{if(!res.headersSent)res.writeHead(502);res.end()})
    res.on('close',()=>upstream.destroy());req.pipe(upstream)
  })
  proxy.listen(0,'127.0.0.1');await once(proxy,'listening')
  const publicOrigin=`https://127.0.0.1:${proxy.address().port}`
  app=await startServer({adminPassword:password,extraArgs:['--public-url',publicOrigin],extraEnv:{DOTBOT_SKILL_DEFAULTS:'0'}})
  browserServer=await chromium.launchServer({headless:true,executablePath:process.env.DOTBOT_CHROME||undefined})
  owned=descendants(browserServer.process().pid)
  browser=await chromium.connect(browserServer.wsEndpoint())
  context=await browser.newContext({viewport:{width:1440,height:1000},locale:'zh-CN',ignoreHTTPSErrors:true})
  page=await context.newPage()
  const errors=[];page.on('pageerror',error=>errors.push(error.message))
  const sensitiveRequests=[]
  page.on('request',request=>{if(new URL(request.url()).search.includes(password))sensitiveRequests.push(request.url())})
  await page.goto(publicOrigin+'/');await page.waitForURL('**/login')
  await page.locator('#login-submit:not([disabled])').waitFor()
  assert.equal(await page.locator('h1').innerText(),'欢迎回来')
  assert.equal(await page.title(),'登录 · dotbot')
  assert.equal(await page.locator('.login-brand span').innerText(),'dotbot')
  assert.equal(await page.locator('a[href*=register],a[href*=signup]').count(),0)
  assert.equal(await page.locator('#username').getAttribute('readonly'),null)
  for(const theme of ['dark','light']){
    await page.evaluate(theme=>document.documentElement.dataset.theme=theme,theme)
    await page.screenshot({path:join(artifacts,`admin-login-${theme}.png`)})
  }
  await page.setViewportSize({width:390,height:844})
  assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth))
  await page.screenshot({path:join(artifacts,'admin-login-mobile.png')})
  await page.fill('#username','admin');await page.fill('#password','wrong');await page.click('#login-submit')
  await page.getByText('管理员账号或密码不正确。',{exact:true}).waitFor()
  await page.fill('#password',password);await page.click('#toggle-password')
  assert.equal(await page.locator('#password').getAttribute('type'),'text')
  await page.click('#toggle-password');await page.check('#remember')
  const [loginResponse]=await Promise.all([page.waitForResponse(response=>response.url().endsWith('/auth/login')),page.click('#login-submit')])
  assert.equal(loginResponse.status(),200)
  assert.match((await loginResponse.allHeaders())['set-cookie'],/Max-Age=2592000; Secure/)
  await page.waitForURL(publicOrigin+'/');await page.locator('#bots .row').first().waitFor()
  assert.equal(await page.title(),'dotbot')
  assert.equal(await page.locator('.brand > span').last().innerText(),'dotbot')
  const cookie=(await context.cookies(publicOrigin)).find(c=>c.name==='teambot')
  assert.ok(cookie.httpOnly&&cookie.secure&&cookie.sameSite==='Strict')
  await page.reload();await page.locator('#bots .row').first().waitFor()
  // Create a distinct authenticated history entry before replacing it on logout.
  await page.goto(publicOrigin+'/?history-check=1');await page.locator('#bots .row').first().waitFor()
  await page.setViewportSize({width:1440,height:1000})
  await page.click('#open-settings');await page.locator('#admin-logout').waitFor()
  assert.equal(await page.locator('#admin-settings #p-avatar .user-avatar').count(),1)
  assert.equal(await page.locator('#p-display').innerText(),'admin')
  await page.locator('.admin-password summary').click()
  await page.screenshot({path:join(artifacts,'admin-profile-desktop.png')})
  assert.equal(await page.locator('#admin-username').inputValue(),'admin')
  await page.fill('#admin-username','team.owner')
  await page.fill('#admin-current',password);await page.fill('#admin-password',replacement);await page.fill('#admin-confirm','different-password-123')
  await page.click('#admin-password-form button[type=submit]')
  await page.getByText('两次输入的新密码不一致。',{exact:true}).waitFor()
  // Passwords must never enter the app's navigation draft cache.
  assert.ok(await page.evaluate(()=>settingsDraft().every(field=>!field.id.startsWith('admin-'))))
  await page.fill('#admin-confirm',replacement);await page.click('#admin-password-form button[type=submit]')
  await page.waitForURL('**/login');await page.locator('#login-submit:not([disabled])').waitFor()
  await page.fill('#username','team.owner')
  await page.fill('#password',replacement);await page.click('#login-submit')
  await page.waitForURL(publicOrigin+'/');await page.locator('#bots .row').first().waitFor()
  await page.click('#open-settings');await page.locator('#admin-identity').waitFor()
  assert.equal(await page.locator('#admin-identity').innerText(),'team.owner')
  await page.locator('.admin-password summary').click()
  await page.fill('#admin-current',replacement);await page.fill('#admin-username','负责人')
  await page.setViewportSize({width:390,height:844})
  await page.locator('#admin-password-form').scrollIntoViewIfNeeded()
  assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth))
  await page.screenshot({path:join(artifacts,'admin-credentials-mobile.png')})
  await page.click('#admin-password-form button[type=submit]')
  await page.waitForURL('**/login');await page.locator('#login-submit:not([disabled])').waitFor()
  await page.fill('#username','负责人');await page.fill('#password',replacement);await page.click('#login-submit')
  await page.waitForURL(publicOrigin+'/');await page.locator('#bots .row').first().waitFor()
  await page.setViewportSize({width:1440,height:1000})
  await page.click('#open-settings');await page.locator('#admin-logout').waitFor();await page.click('#admin-logout')
  await page.waitForURL('**/login');await page.goBack()
  await page.waitForURL(url=>url.pathname==='/login')
  await page.locator('#login-form').waitFor()
  assert.equal((await page.request.post(publicOrigin+'/api/bot',{data:{method:'team.state'}})).status(),401)
  // Reuse the same browser/context/page for the first-install wizard.
  await app.stop()
  app=await startServer({bootstrap:true,storage:'sqlite',extraArgs:['--public-url',publicOrigin],extraEnv:{DOTBOT_SKILL_DEFAULTS:'0'}})
  await context.clearCookies()
  await page.goto(publicOrigin+'/');await page.waitForURL('**/login')
  await page.locator('#login-submit:not([disabled])').waitFor()
  assert.equal(await page.locator('h1').innerText(),'开始设置 dotbot')
  await page.fill('#password',app.initialPassword);await page.click('#login-submit')
  await page.waitForURL('**/setup');await page.locator('#setup-submit:not([disabled])').waitFor()
  assert.equal((await page.request.post(publicOrigin+'/api/bot',{data:{method:'team.state'}})).status(),401)
  await page.setViewportSize({width:390,height:844})
  await page.click('#setup-submit')
  await page.getByText('新密码须至少 6 个字符，且不超过 256 字节。',{exact:true}).waitFor()
  await page.fill('#setup-username','首次管理员')
  await page.fill('#setup-password',replacement);await page.fill('#setup-confirm',replacement)
  await page.screenshot({path:join(artifacts,'setup-account-mobile.png')})
  await page.click('#setup-submit');await page.locator('#setup-name').waitFor()
  await page.fill('#setup-name','小林');await page.fill('#setup-bio','我主要从事研究和开发，偏好中文沟通。')
  await page.click('#setup-back')
  assert.equal(await page.locator('#setup-password').inputValue(),replacement)
  await page.click('#setup-submit')
  assert.equal(await page.locator('#setup-name').inputValue(),'小林')
  assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth))
  for(const theme of ['light','dark']){
    await page.evaluate(theme=>document.documentElement.dataset.theme=theme,theme)
    await page.screenshot({path:join(artifacts,`setup-profile-${theme}-mobile.png`)})
  }
  await page.click('#setup-submit');await page.waitForURL('**/login?welcome=1')
  await page.locator('#login-submit:not([disabled])').waitFor()
  assert.equal(await page.locator('h1').innerText(),'设置完成')
  await page.fill('#username','首次管理员');await page.fill('#password',replacement);await page.click('#login-submit')
  await page.waitForURL(publicOrigin+'/');await page.locator('#bots .row').first().waitFor()
  const state=await (await page.request.post(publicOrigin+'/api/bot',{data:{method:'team.state'}})).json()
  assert.equal(state.value.prefs.profile.name,'小林')
  await page.goto(publicOrigin+'/setup');await page.waitForURL(publicOrigin+'/')
  assert.deepEqual(sensitiveRequests,[]);assert.deepEqual(errors,[])
  console.log('ok   real HTTPS administrator login, Secure cookie, combined username/password changes, Chinese username-only changes, logout and back-navigation protection')
  console.log('ok   desktop/mobile layouts, dark/light themes, accessible errors; passwords excluded from URLs and app drafts')
  console.log('ok   first-install wizard, back/next draft preservation, forced new password and saved profile on mobile layouts')
} finally{
  try{owned=[...new Set([...owned,...(browserServer?descendants(browserServer.process().pid):[])])]}catch{}
  await page?.close().catch(()=>{});await context?.close().catch(()=>{});await browser?.close().catch(()=>{});await browserServer?.close().catch(()=>{})
  await app?.stop()
  if(proxy){proxy.closeAllConnections();await new Promise(resolve=>proxy.close(resolve))}
  for(const pid of owned)if(alive(pid))try{process.kill(pid,'SIGTERM')}catch{}
  await new Promise(resolve=>setTimeout(resolve,250))
  for(const pid of owned)if(alive(pid))try{process.kill(pid,'SIGKILL')}catch{}
  await new Promise(resolve=>setTimeout(resolve,100))
  const remaining=owned.filter(alive)
  rmSync(directory,{recursive:true,force:true})
  assert.deepEqual(remaining,[],'owned browser processes must all exit')
}
