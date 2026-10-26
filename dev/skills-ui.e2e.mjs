#!/usr/bin/env node
// One browser/context/page; background imports use a local GitHub API fixture.
import assert from 'node:assert/strict'
import { chromium } from 'playwright'
import { createServer } from 'node:http'
import { execFileSync } from 'node:child_process'
import { readFileSync, mkdirSync } from 'node:fs'
import { join } from 'node:path'
import { startServer, until, root } from './harness.mjs'
const sha='1'.repeat(40), license='2'.repeat(40)
const source=createServer((req,res)=>{
  res.setHeader('content-type','application/json')
  if(req.url.includes('/trees/')) res.end(JSON.stringify({sha:'a'.repeat(40),tree:[
    {path:'skills/frontend-design/SKILL.md',type:'blob',mode:'100644',sha},
    {path:'LICENSE',type:'blob',mode:'100644',sha:license}
  ]}))
  else setTimeout(()=>res.end(JSON.stringify({sha:req.url.split('/').at(-1),encoding:'base64',content:Buffer.from(req.url.endsWith(license)?'Fixture license':'---\nname: frontend-design\ndescription: Design usable responsive frontends\n---\n\nInspect existing styles, implement responsive layouts, and check text, controls and overflow.').toString('base64')})),120)
})
let app,server,browser,context,page,owned=[]
const descendants=pid=>{
 const rows=execFileSync('ps',['-eo','pid=,ppid=']).toString().trim().split('\n').map(x=>x.trim().split(/\s+/).map(Number)),set=new Set([pid]);let count
 do{count=set.size;for(const [id,parent] of rows)if(set.has(parent))set.add(id)}while(count!==set.size)
 return [...set]
}
const running=pid=>{try{return readFileSync(`/proc/${pid}/stat`,'utf8').split(') ')[1][0]!=='Z'}catch{return false}}
try{
 await new Promise(resolve=>source.listen(0,'127.0.0.1',resolve))
 app=await startServer({extraEnv:{DOTBOT_LEARNING_DELAY_MS:'300',DOTBOT_SKILL_API_BASE:`http://127.0.0.1:${source.address().port}`}})
 await app.call('settings.lang',{lang:'zh'})
 const chief=(await app.call('team.state')).bots[0]
 server=await chromium.launchServer({headless:true,executablePath:process.env.DOTBOT_CHROME||undefined});owned=descendants(server.process().pid)
 browser=await chromium.connect(server.wsEndpoint());context=await browser.newContext({viewport:{width:1280,height:900}});page=await context.newPage()
 const errors=[];page.on('pageerror',e=>errors.push(e.message))
 let catalogRequests=0
 await page.route('**/api/bot',async route=>{
  if(route.request().postDataJSON().method==='skill.catalog'){
   catalogRequests++
   await route.fulfill({status:503,contentType:'application/json',body:JSON.stringify({ok:false,error:{message:'Catalog unavailable'}})})
  }else await route.continue()
 })
 await app.authorize(context); await page.goto(app.address);await page.locator('#bots .row').first().waitFor()
 await page.locator('#open-settings').click();await page.locator('[data-page="skills"]').click();await page.locator('[data-plugin-tab="manual"]').click()
 await page.locator('#sk-name').fill('未保存草稿');await page.locator('#sk-text').fill('必须保留的编辑内容')
 assert.equal(await page.locator('.skill-catalog, [data-skill-install]').count(),0)
 assert.equal(await page.locator('#settings-content').getAttribute('aria-busy'),'false')
 // A background import still appears as an installed skill, without a catalog UI.
 await app.call('skill.install',{source:'anthropic/frontend-design'})
 await page.waitForFunction(()=>view.team.skills.some(s=>s.name==='frontend-design'))
 await page.locator('[data-page="general"]').click();await page.locator('[data-page="skills"]').click()
 assert.equal(await page.locator('#sk-text').inputValue(),'必须保留的编辑内容')
 await page.locator('[data-plugin-tab="installed"]').click()
 await page.locator('[data-detail="skill:frontend-design"]').waitFor()
 assert.equal(catalogRequests,0,'skill management must not request the public catalog')
 assert.equal((await app.call('skill.get',{name:'frontend-design'})).origin.validation,'untested')
 await page.locator('#ls-skills-installed').fill('frontend-design')
 assert.equal(await page.locator('[data-list-body="skills-installed"] .mini-card').count(),1)
 await page.locator('[data-detail="skill:frontend-design"]').click()
 assert.ok((await page.locator('#modal-card').textContent()).includes('公开导入'))
 assert.ok((await page.locator('#modal-card').textContent()).includes('尚未验证'))
 await page.locator('#modal-card [data-close]').click()
 await page.setViewportSize({width:390,height:844})
 assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1))
 mkdirSync(join(root,'test-results'),{recursive:true})
 await page.screenshot({path:join(root,'test-results/skills-installed-mobile.png'),fullPage:true,animations:'disabled'})
 await page.locator('#settings-close').click()
 await page.locator('#settings').waitFor({state:'hidden'})
 assert.equal(await page.locator('#panel').isHidden(),true,'no Workspace panel opens by itself')
 await page.locator('#conversation-menu > summary').click();await page.locator('#act-details').click()
 // Bots choose skills themselves: the details panel has no skill controls or learning records.
 await page.locator('#panel-body #f-name').waitFor()
 assert.equal(await page.locator('.bot-skill-settings, [data-bot-skill], .task-review-settings, .task-review, .bot-performance').count(),0)
 assert.doesNotMatch(await page.locator('#panel-body').textContent(),/技能专长|复盘|任务表现与分工依据/)
 assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1))
 await page.screenshot({path:join(root,'test-results/bot-details-mobile.png'),fullPage:true,animations:'disabled'})
 await page.locator('#panel-close').click()
 await page.locator('#toggle-sidebar').click();await page.locator('#open-settings').click();await page.locator('[data-page="skills"]').click()
 const toggle=page.locator('#learning-enabled')
 assert.equal(await toggle.isChecked(),false,'Curator is off by default')
 assert.match(await page.locator('.learning-toggle').textContent(),/整理员（Curator）/)
 assert.doesNotMatch(await page.locator('#settings-content').textContent(),/复盘/)
 await toggle.check()
 await until('Curator enabled',async()=>(await app.call('team.state')).prefs.learningEnabled===true)
 await toggle.uncheck()
 await until('Curator disabled',async()=>(await app.call('team.state')).prefs.learningEnabled===false)
 console.log('ok   Bot details have no skill controls; the optional Curator switch is named Hermes-style, off by default and persists')
 assert.deepEqual(errors,[])
 assert.equal(catalogRequests,0)
 assert.equal(await page.locator('.skill-catalog, [data-skill-install]').count(),0)
 console.log('ok   public catalog and import controls are absent; installed skills load without catalog requests, retain drafts/provenance and support mobile Bot preferences')
 // MCP store: the 300 most used servers from ModelScope's MCP 广场, Chinese throughout, added from their own data.
 await page.setViewportSize({width:1280,height:900});await page.goto(app.address);await page.locator('#bots .row').first().waitFor()
 await page.locator('#open-settings').click();await page.locator('[data-page="skills"]').click();await page.locator('[data-plugin-tab="mcp"]').click()
 await page.locator('[data-list-body="mcp-store"] .mini-card').first().waitFor()
 const store=await app.call('mcp.store')
 assert.match(store.source,/ModelScope/);assert.equal(store.servers.length,300)
 assert.ok(store.servers.every(x=>/[一-鿿]/.test(x.desc)),'every description is Chinese')
 assert.equal(await page.locator('#settings-content a[href*="github.com"]').count(),0,'the store no longer points at GitHub')
 assert.match(await page.locator('.settings-section-title').filter({hasText:'MCP'}).textContent(),/共 300 个.*魔搭社区 MCP 广场/)
 // Pages of 12 cards, most used first; categories carry their counts and reset the page.
 assert.equal(await page.locator('[data-list-body="mcp-store"] .mini-card').count(),12)
 const firstPage=await page.locator('[data-list-body="mcp-store"] .mini-main strong').allTextContents()
 assert.ok(firstPage.some(name=>name.includes('高德地图')),'the most used domestic servers lead')
 await page.locator('[data-list-page^="mcp-store:"]').last().click()
 assert.equal(await page.locator('[data-list-body="mcp-store"] .mini-card').count(),12)
 assert.notDeepEqual(await page.locator('[data-list-body="mcp-store"] .mini-main strong').allTextContents(),firstPage)
 assert.match(await page.locator('[data-list-body="mcp-store"] .pager').textContent(),/2 \/ 25/)
 await page.locator('[data-mcp-category="map"]').click()
 assert.match(await page.locator('[data-list-body="mcp-store"] .pager-count, [data-list-body="mcp-store"] .pager').textContent(),/1 \/ 2|共 \d+ 项/)
 assert.ok((await page.locator('[data-list-body="mcp-store"] .mini-main small').allTextContents()).every(text=>!text.includes('开发者工具')))
 await page.locator('[data-mcp-category="all"]').click()
 await page.locator('#ls-mcp-store').fill('高德地图')
 assert.ok(await page.locator('[data-list-body="mcp-store"] .mini-card').count()>=1)
 // The details window explains what it is, what it runs and what it needs.
 await page.locator('[data-list-body="mcp-store"] .mini-card').filter({has:page.locator('strong',{hasText:/^高德地图/})}).first().locator('[data-detail^="mcpstore:"]').click()
 const detail=page.locator('#modal-card')
 for(const text of ['地图与出行','运行的命令','npx -y @amap/amap-maps-mcp-server','需要的设置','AMAP_MAPS_API_KEY','必填','需要装有 Node.js','在魔搭查看'])assert.ok((await detail.textContent()).includes(text),text)
 assert.equal(await detail.locator('a[href^="https://www.modelscope.cn/mcp/servers/"]').count(),1)
 // Adding asks only for what is missing, shows the exact command and refuses an empty key.
 await detail.locator('[data-mcp-add]').click()
 assert.ok((await detail.textContent()).includes('将运行'));assert.ok((await detail.textContent()).includes('请只添加你信任的服务'))
 await page.locator('#mi-save').click()
 await page.locator('#toast').filter({hasText:'AMAP_MAPS_API_KEY'}).waitFor()
 await page.locator('#mi-env-0').fill('fixture-amap-key')
 await page.locator('#mi-save').click()
 const amap=await until('Amap connection saved',async()=>(await app.call('team.state')).connections.find(c=>c.command==='npx'&&c.args.includes('@amap/amap-maps-mcp-server')))
 assert.equal(amap.name,'amap-maps');assert.deepEqual(amap.args,['-y','@amap/amap-maps-mcp-server']);assert.equal(amap.env.AMAP_MAPS_API_KEY,'${AMAP_MAPS_API_KEY}','the key is a reference, not the value')
 assert.ok(!JSON.stringify(await app.call('team.state')).includes('fixture-amap-key'),'the key value never reaches the page state')
 await page.locator('[data-plugin-tab="installed"][aria-selected="true"]').waitFor()
 await page.locator('[data-detail="mcp:amap-maps"]').waitFor()
 assert.ok((await page.locator('[data-detail="mcp:amap-maps"]').textContent()).includes('高德地图'),'an added server shows its Chinese name')
 // A server without settings is one click; one whose arguments hold placeholders asks for real values.
 await page.locator('[data-plugin-tab="mcp"]').click();await page.locator('#ls-mcp-store').fill('必应搜索中文')
 await page.locator('[data-list-body="mcp-store"] [data-mcp-add]').first().click();await page.locator('#mi-save').click()
 await until('Bing connection saved',async()=>(await app.call('team.state')).connections.some(c=>c.args?.includes('bing-cn-mcp')))
 await page.locator('[data-plugin-tab="mcp"]').click()
 await page.waitForFunction(()=>document.querySelector('[data-mcp-category="added"] .chip-count')?.textContent==='2')
 await page.locator('#ls-mcp-store').fill('');await page.locator('[data-mcp-category="added"]').click()
 assert.equal(await page.locator('[data-list-body="mcp-store"] .mini-card').count(),2)
 await page.locator('[data-mcp-category="all"]').click();await page.locator('#ls-mcp-store').fill('')
 const holed=store.servers.find(x=>x.kind==='stdio'&&x.args.some(a=>/path\/to/.test(a)))
 await page.locator('#ls-mcp-store').fill(holed.name);await page.locator(`[data-mcp-add="${holed.id}"]`).first().click()
 await page.locator('#mi-save').click()
 await page.locator('#toast').filter({hasText:'改成实际的值'}).waitFor()
 await page.locator('#modal-card [data-close]').first().click()
 // Pasting a README's configuration fills the manual form.
 await page.locator('[data-plugin-tab="manual"]').click()
 await page.locator('#cn-paste').fill('```json\n{"mcpServers":{"Demo Server":{"command":"npx","args":["-y","demo-mcp","--flag","a b"],"env":{"DEMO_TOKEN":"abc"}}}}\n```')
 await page.locator('#cn-paste-go').click()
 assert.equal(await page.locator('#cn-name').inputValue(),'demo-server');assert.equal(await page.locator('#cn-kind').inputValue(),'stdio')
 assert.equal(await page.locator('#cn-target').inputValue(),'npx -y demo-mcp --flag "a b"');assert.equal(await page.locator('#cn-pairs').inputValue(),'DEMO_TOKEN=abc')
 await page.locator('#cn-paste').fill('claude mcp add --transport http linear https://mcp.linear.app/mcp')
 await page.locator('#cn-paste-go').click()
 assert.equal(await page.locator('#cn-kind').inputValue(),'http');assert.equal(await page.locator('#cn-target').inputValue(),'https://mcp.linear.app/mcp')
 await page.locator('#cn-paste').fill('hello')
 await page.locator('#cn-paste-go').click()
 await page.locator('#toast').filter({hasText:'无法识别'}).waitFor()
 // Phones: the cards stay inside the screen.
 await page.setViewportSize({width:390,height:844});await page.locator('[data-plugin-tab="mcp"]').click()
 assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1))
 await page.screenshot({path:join(root,'test-results/mcp-store-mobile.png'),fullPage:true,animations:'disabled'})
 assert.deepEqual(errors,[])
 console.log('ok   MCP store: 300 Chinese-described domestic servers from ModelScope, categories, search, paging, details, guided add with keys kept out of the page, and pasted configurations')
}finally{
 if(server&&running(server.process().pid))owned=descendants(server.process().pid)
 await page?.close().catch(()=>{});await context?.close().catch(()=>{});await browser?.close().catch(()=>{});await server?.close().catch(()=>{})
 for(const pid of owned.filter(running)){try{process.kill(pid,'SIGTERM')}catch{}}
 let remaining=[]
 try{await until('owned browser processes closed',()=>owned.every(pid=>!running(pid)),5000)}catch{remaining=owned.filter(running)}
 await app?.stop();if(source.listening)await new Promise(resolve=>source.close(resolve))
 assert.deepEqual(remaining,[],`Browser cleanup failed for PIDs: ${remaining.join(', ')}`)
 console.log('ok   task-owned browser, test service and source fixture closed')
}
