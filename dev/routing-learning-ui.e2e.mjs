#!/usr/bin/env node
import assert from 'node:assert/strict'
import {chromium} from 'playwright'
import {execFileSync} from 'node:child_process'
import {readFileSync,mkdirSync} from 'node:fs'
import {join} from 'node:path'
import {startServer,until,root} from './harness.mjs'
let app,server,browser,context,page,owned=[]
const descendants=pid=>{const rows=execFileSync('ps',['-eo','pid=,ppid=']).toString().trim().split('\n').map(r=>r.trim().split(/\s+/).map(Number)),ids=new Set([pid]);let n;do{n=ids.size;for(const[id,parent]of rows)if(ids.has(parent))ids.add(id)}while(n!==ids.size);return [...ids]}
const running=pid=>{try{return readFileSync(`/proc/${pid}/stat`,'utf8').split(') ')[1][0]!=='Z'}catch{return false}}
try{
  app=await startServer({binary:process.env.DOTBOT_TEST_BINARY||undefined,extraEnv:{DOTBOT_SKILL_DEFAULTS:'0',DOTBOT_LEARNING_DELAY_MS:'300'}})
  const {call}=app;await call('settings.lang',{lang:'zh'});await call('settings.local-computer',{enabled:true});await call('settings.learning',{enabled:true}) // the Curator is opt-in
  const chief=(await call('team.state')).bots[0]
  await call('skill.save',{name:'records',description:'校验记录数量',text:'比较实际输入与输出数量，生成 JSON 产物并检查。'})
  const command=`node -e 'require("fs").writeFileSync("checked.json",JSON.stringify({count:3}))'`
  await call('skill.benchmark.save',{name:'records',goal:`TOOL local_run ${JSON.stringify({command})}\nTOOL local_send_file {"path":"__TASK_CWD__/checked.json"}`,acceptance:'文件可解析且 count 为 3。'})
  await call('chat.send',{conversation:chief.conversation,text:'AUTO_LEARN_SUCCESS uiproof\nTOOL read_skill {"name":"records"}\nTOOL local_run {"command":"printf verified"}'})
  const review=await until('candidate created',async()=>{for(const a of (await call('team.state')).work.approvals)await call('approval.answer',{id:a.id,allow:true});return(await call('learning.list',{bot:chief.id})).find(r=>r.candidate)},25000)
  server=await chromium.launchServer({headless:true,executablePath:process.env.DOTBOT_CHROME||undefined});owned=descendants(server.process().pid)
  browser=await chromium.connect(server.wsEndpoint());context=await browser.newContext({viewport:{width:1280,height:900},locale:'zh-CN'});page=await context.newPage()
  const errors=[];page.on('pageerror',e=>errors.push(e.message));await app.authorize(context); await page.goto(app.address)
  await page.evaluate(()=>showSkillCandidates('records'));await page.locator('[data-candidate-test]').waitFor()
  assert.ok((await page.locator('#modal').textContent()).includes('正式版本'))
  await page.locator('[data-candidate-test]').click()
  await until('UI candidate promoted',async()=>{for(const a of(await call('team.state')).work.approvals)await call('approval.answer',{id:a.id,allow:true});return(await call('skill.candidates',{name:'records'})).some(c=>c.id===review.candidate&&c.status==='promoted')},40000)
  await page.evaluate(()=>showSkillCandidates('records'));await page.locator('.candidate-card').waitFor()
  assert.ok((await page.locator('#modal').textContent()).includes('已通过并启用'))
  assert.equal(await page.locator('[data-candidate-test]').count(),0)
  const artifacts=join(root,'test-results');mkdirSync(artifacts,{recursive:true})
  await page.screenshot({path:join(artifacts,'learning-candidate-desktop.png'),animations:'disabled'})
  await page.setViewportSize({width:390,height:844});await page.waitForFunction(()=>document.querySelector('#sidebar').getBoundingClientRect().right<=1)
  assert.ok(await page.locator('#modal').evaluate(el=>el.scrollWidth<=el.clientWidth+1))
  await page.screenshot({path:join(artifacts,'learning-candidate-mobile.png'),animations:'disabled'})
  assert.deepEqual(errors,[]);console.log('ok   Chinese candidate panel starts paired tests, shows promotion evidence and fits desktop/mobile without script errors')
}finally{
  if(server&&running(server.process().pid))owned=[...new Set([...owned,...descendants(server.process().pid)])]
  try{await page?.close()}finally{try{await context?.close()}finally{try{await browser?.close()}finally{await server?.close()}}}
  for(const pid of owned.filter(running)){try{process.kill(pid,'SIGTERM')}catch{}}
  let remaining=[];try{await until('browser cleanup',()=>owned.every(pid=>!running(pid)),5000)}catch{remaining=owned.filter(running)}
  await app?.stop();assert.deepEqual(remaining,[],`Task-owned browser PIDs remain: ${remaining}`);console.log('ok   all task-owned browser sessions/processes and test service cleaned up')
}
