// A stand-in model service for tests: it speaks the Anthropic Messages API that dotbot's
// native engine calls, needs no key, and asks the engine to run real team tools. A prompt
// line
//   TOOL <name> <json args>
// makes the model call that team tool (or, named mcp__<server>__<tool>, a connection's tool) and report the result; any other prompt is echoed back.
// A structured request (tool_choice structured_answer) answers one question the way the
// memory review, planning and verification jobs ask: a line "remember: <fact>" becomes
// an entry. Each model run appends a Claude-CLI-shaped argument list (--model,
// --system-prompt, --json-schema, --resume) to `log`, so tests can inspect what was sent.
import { appendFileSync as append, existsSync, writeFileSync } from 'node:fs'
import { createServer } from 'node:http'
import { randomUUID } from 'node:crypto'

// A request still in flight when a test removes its folder must not crash the test process.
const appendFileSync = (path, text) => { try { append(path, text) } catch {} }
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms))

function structured(schema, text) {
  let answer
  if (schema.properties.verification) {
    const input=JSON.parse(text)
    answer={verification:{passed:input.tasks.every(t=>t.status==='completed'),summary:'已按实际任务结果逐项核对。',checks:input.tasks.map(task=>({task:task.id,passed:task.status==='completed'&&Boolean(task.result),reason:task.status==='completed'?'产物和任务结果已返回。':'任务未成功，需要修复。'}))}}
    if(input.goal.includes('VERIFY_BAD'))answer.verification.checks=[]
  }
  else if (schema.properties.repair) {
    const input=JSON.parse(text), retry=/AUTO_REPAIR/.test(input.goal)
    const other=input.candidates.find(b=>b.id!==input.task.target)
    answer={repair:{retry,bot:input.goal.includes('REASSIGN')?other?.id||input.task.target:input.task.target,diagnosis:'执行发生可恢复故障，已保留上下文。',plan:'核对原始附件和检查点，改由合适的 Bot 继续完成当前步骤。'}}
  }
  else if (schema.properties.experienceSelection) {
    const input=JSON.parse(text),token=input.request.match(/USE_EXPERIENCE ([\w-]+)/)?.[1]
    answer={experienceSelection:input.candidates.filter(c=>token?c.trigger.includes(token):c.trigger.includes('validation')&&/核对|校验|检查/.test(input.request)).slice(0,6).map(c=>c.id)}
  }
  else if (schema.properties.reflection) {
    const input=JSON.parse(text), marker=input.request.match(/AUTO_LEARN_(SUCCESS|FAILURE|SLOW|INVALID) ([\w-]+)/)
    let reflection={summary:'',lessons:[],skillUpdate:null}
    if(marker){
      const [,kind,token]=marker
      const action=input.actions.find(a=>a.tool==='local_run'&&a.state==='confirmed')
      const skill=input.usedSkills[0]
      reflection={summary:`Task retrospective ${token}: ${input.outcome}`,lessons:[{trigger:`${token} validation`,method:`For ${token}, compare the generated output with source records before delivery; retain the check outcome and stop when inputs are missing.`,evidence:kind==='INVALID'?['invented-evidence']:[action?.id||'result']}],skillUpdate:skill&&action?{name:skill.name,revision:skill.revision,improvement:`For ${token}, verify output record counts against the input before delivery; preserve the counts as evidence and investigate mismatches.`,evidence:[action.id]}:null}
      if(kind==='SLOW') return { answer: { reflection }, delay: 10000 }
    }
    answer={reflection}
  }
  else if (schema.properties.skillPlan) {
    const input=JSON.parse(text), marker=input.request.match(/AUTO_SKILL_(CREATE|INSTALL|INVALID|SLOW|USE) ([\w-]+)/)
    let plan={action:'none',names:[],source:'',draft:null}
    if (marker) {
      const [,kind,token]=marker
      const name=kind==='USE' ? token : kind==='INSTALL' ? token : `workflow-${token}-${input.bot.id.slice(-8)}`
      const existing=input.installed.find(s=>s.name===name && s.enabled)
      const draft={name:kind==='INSTALL' ? `fallback-${token}-${input.bot.id.slice(-8)}` : name,description:`${input.bot.role} workflow for ${token}`,body:`## Goal\nHandle ${token} as ${input.bot.role}; parameterize the actual task.\n## Steps\nInspect task inputs and available tools, collect evidence, produce the requested artifact.\n## Checks\nCheck the actual output against the request and cite evidence.\n## Recovery\nStop on missing inputs, preserve progress and report limits.\n## Deliverable\nReturn the artifact and validation notes. AUTO_SKILL_WORKFLOW ${token}`}
      if (kind==='INVALID') plan={action:'use',names:['invented-skill'],source:'',draft:null}
      else if (kind!=='SLOW') plan=existing ? {action:'use',names:[name],source:'',draft:null} : {action:kind==='INSTALL'?'install':'create',names:[],source:kind==='INSTALL'?`anthropic/${token}`:'',draft}
      if (kind==='SLOW') return { answer: { skillPlan: plan }, delay: 10000 }
    }
    answer={skillPlan:plan}
  }
  else if (schema.properties.messageTriage) {
    // A message during work amends it unless it says TRIAGE_NEW / TRIAGE_PROGRESS / TRIAGE_STOP.
    const input = JSON.parse(text), m = input.newMessage.match(/TRIAGE_(NEW|PROGRESS|STOP)/)?.[1]
    const kind = { NEW: 'new', PROGRESS: 'progress', STOP: 'stop' }[m] ?? 'amend'
    answer = { messageTriage: { kind, answer: kind === 'progress' ? `PROGRESS_ANSWER ${input.doingNow.filter(Boolean).join(' ')} after ${input.elapsedSeconds}s` : '' } }
  }
  else if (schema.properties.claim) {
    // A group member claims a part only when the request names it with CLAIM_<name>.
    const input = JSON.parse(text), token = input.request.match(new RegExp(`CLAIM_${input.me.name} (\\w+)`))?.[1]
    answer = token ? { claim: true, part: `CLAIMED_PART ${token}` } : { claim: false, part: '' }
  }
  else if (schema.properties.delegationChoice) {
    const input = JSON.parse(text)
    const named = input.request.match(/REUSE_BOT ([\w-]+)/)?.[1]
    const media = /IMAGE_PRIVACY_CASE/.test(input.request)
    const candidate = named ? input.candidates.find(b => b.id === named)
      : input.request.includes('NO_MATCH') ? null
      : input.candidates.find(b => (media ? /生图|image/i : /review|核查/i).test(`${b.role} ${b.description ?? ''}`))
    answer = {delegationChoice:{bot:input.request.includes('DELEGATION_INVALID') ? 'invented-bot' : candidate?.id ?? null, requires_write:input.request.includes('REQUIRES_WRITE')}}
    if (input.request.includes('DELEGATION_SLOW')) return { answer, delay: 10000 }
  }
  else if (schema.properties.assignments) {
    const input = JSON.parse(text)
    answer = { assignments: [] }
    let delay = 0
    // Allocation is driven only by this request, never by stale conversation history.
    const marker = input.request.match(/AUTO_COORDINATE(?:_(INVALID|SLOW|FAIL))? (\w+)/)
    if (marker && !/work alone|独自完成|不要分工|不要委派/i.test(input.request)) {
      if (marker[1] === 'SLOW') delay = 10000
      const specialty = input.request.match(/SKILL_ROUTING ([\w-]+)/)?.[1]
      const ranked=input.request.includes('PERFORMANCE_ROUTING')?[...input.candidates].sort((a,b)=>(b.performance?.verifiedPasses||0)-(a.performance?.verifiedPasses||0)||(a.performance?.running||0)-(b.performance?.running||0)):input.candidates
      const candidate = specialty ? ranked.find(bot => bot.skills?.some(s => s.name === specialty && s.enabled)) : ranked.find(bot => /review/i.test(bot.role))
      if (candidate) answer.assignments.push({ bot: marker[1] === 'INVALID' ? 'invented-bot' : candidate.id, task: `${marker[1] === 'FAIL' ? `FAIL allocated-failure\nAUTO_RESULT ${marker[2]}` : `DELAY ${input.request.includes('HOLD_WORK') ? 6000 : 1200}\nAUTO_RESULT ${marker[2]}`}`, expected_output: `Evidence for ${marker[2]}`, acceptance: 'Include verified findings', requires_write: false, files: input.request.includes('SHARE_FILE') ? input.attachments.map(f => f.src) : [] })
      if (input.request.includes('PARALLEL_WORK')) answer.assignments = input.candidates.slice(0, 4).map((bot, index) => ({bot:bot.id, task: index === 0 && input.request.includes('FAIL_FIRST') ? 'FAIL parallel-worker-failed' : `DELAY 8000\nAUTO_RESULT ${marker[2]}_worker${index}`, expected_output:`Independent evidence ${index}`, acceptance:'Return only this worker result', requires_write:false, files:input.request.includes('SHARE_FILE') ? input.attachments.map(f=>f.src) : []}))
    }
    if (input.request.includes('INHERIT_FILES')) for (const assignment of answer.assignments) delete assignment.files
    const flow=input.request.match(/AUTO_WORKFLOW (\w+)/)
    if(flow&&input.candidates.length>=2){
      const [first,second]=input.candidates
      const task=(id,bot,depends_on=[])=>({id,bot:bot.id,depends_on,priority:0,task:`DELAY ${input.request.includes('HOLD_FLOW')?8000:1200}\n${id==='a'&&input.request.includes('FAIL_CHAIN')?`FAIL_ONCE ${flow[1]}`:''}${id==='a'&&input.request.includes('FAIL_ALWAYS')?'\nFAIL permanent-fixture':''}\nWORKFLOW_RESULT ${flow[1]}_${id}\n${input.request.includes('PROJECT_CASE')?'PROJECT_CASE':''}`,expected_output:`步骤 ${id} 的实际结果`,acceptance:'返回本步骤结果和前置步骤引用',requires_write:input.request.includes('PROJECT_CASE'),files:[]})
      answer.assignments=input.request.includes('SAME_BOT')?['a','b','c','d'].map(id=>task(id,first)):[task('a',first),task('b',first),task('c',second,['a','b'])]
    }
    return { answer, delay }
  }
  else if (schema.properties.summary) answer = { summary: `## What is saved\n${(text.match(/^- /gm) ?? []).length} entries.` }
  else {
    const facts = [...text.matchAll(/remember: (.+)$/gim)].map(match => match[1].trim())
    answer = { changes: facts.map(fact => ({ op: 'add', text: fact })) }
    if (schema.properties.reply) answer.reply = `Noted ${facts.length}.`
  }
  return { answer, delay: 0 }
}

const blocksOf = message => typeof message.content === 'string' ? [{ type: 'text', text: message.content }] : message.content
const isPrompt = message => message.role === 'user' && blocksOf(message).some(b => b.type !== 'tool_result')

/// What the turn plans from its prompt: the tool lines to call, in order, and the
/// fixture failures it acts out. Recomputed on every request, so the server keeps no state.
function plan(prompt, system, options) {
  const correcting = prompt.includes('[当前任务纠正]') || prompt.includes('[用户纠正当前任务，以最新要求为准]') || prompt.includes('[任务进行中收到的新消息]') || prompt.includes('[用户在任务进行中发来的新消息')
  const recovering = prompt.includes('[Recovery review · attempt ')
  const delegated = prompt.includes('# 自动分派的任务') || prompt.includes('# 已指派的协作任务')
  const unified = prompt.includes('[工作流统一验收与交付]')
  const executionPrompt = prompt.replace(/\n\n\[本轮群聊任务\]\n[\s\S]*?\n\[\/本轮群聊任务\]/g, '')
  const calls = recovering || unified || delegated ? [] : executionPrompt.split('\n').map(line => line.replace(/^[^:\n]+: (TOOL )/, '$1').replace(/^任务：(?=TOOL )/,'')).filter(line => line.startsWith('TOOL '))
  const imagePrivacy = /IMAGE_PRIVACY_CASE(?:_FAIL)?/.test(prompt) && !delegated && !unified
  const videoPrivacy = /VIDEO_PRIVACY_CASE(?:_FAIL)?/.test(prompt) && !delegated && !unified
  const generationPrompt = videoPrivacy ? 'PRIVATE_VIDEO_PROMPT_91d2 motion camera trajectory negative-prompt internal-render-settings' : 'PRIVATE_IMAGE_PROMPT_7e81 lighting composition negative-prompt internal-render-settings'
  if (imagePrivacy || videoPrivacy) calls.push(`TOOL ${videoPrivacy ? 'generate_video' : 'generate_image'} ${JSON.stringify({prompt:generationPrompt,size:videoPrivacy ? '1280x720' : '1024x1024'})}`)
  // Continue a synthetic parent workflow after its child result, using only the
  // task context supplied by the real server (not shared fixture memory).
  if (!recovering && prompt.startsWith('[Reply from ')) {
    try {
      const contexts = JSON.parse(system.split('# 委派上下文\n')[1]?.split('\n')[1])
      for (const item of contexts) {
        const next = item.parentRequest?.match(/^AFTER_CHILD (TOOL .+)$/m)?.[1]
        if (next) calls.push(next)
      }
    } catch {}
  }
  if(prompt.includes('[桌面示范教学]'))calls.push(`TOOL save_skill ${JSON.stringify({name:'desktop-demo-fixture',description:'在 Bot 桌面浏览器和应用中示范任务',steps:'## 参数\n{{网站地址}}、{{输入值}}\n## 步骤\n先截图确认应用和布局，再操作目标窗口。输入使用任务参数；保存产物并检查实际结果。失败时保留检查点，涉及发布先按权限核对。'})}`)
  if((prompt.includes('[技能试运行]')||prompt.includes('[技能候选对照评测]'))&&!unified){
    const skill=prompt.match(/请先 read_skill 读取 ([^，\s]+)/)?.[1]
    if(skill)calls.unshift(`TOOL read_skill ${JSON.stringify({name:skill})}`)
    else if(prompt.includes('[技能试运行]')){const name=prompt.match(/读取 ([a-z0-9-]+)/)?.[1];if(name)calls.push(`TOOL read_skill ${JSON.stringify({name})}`)}
  }
  if(prompt.includes('[技能候选对照评测]')&&prompt.includes('REGRESSION_FAIL_CANDIDATE')){
    const content=options.pluginDir?.endsWith('/candidate')?'broken-json':'{}'
    calls.push('TOOL local_run '+JSON.stringify({command:`node -e 'require("fs").writeFileSync("regression-bad.json",${JSON.stringify(content)})'`}))
    calls.push('TOOL local_send_file '+JSON.stringify({path:`${options.cwd}/regression-bad.json`}))
  }
  if(prompt.includes('PROJECT_CASE')&&!unified&&/^WORKFLOW_RESULT (\w+)$/m.test(prompt)){
    const id=prompt.match(/^WORKFLOW_RESULT (\w+)$/m)[1].slice(-1)
    if(id==='a'||id==='b')calls.push(`TOOL local_run ${JSON.stringify({command:`node -e 'const fs=require("fs");let s=fs.readFileSync("shared.txt","utf8");fs.writeFileSync("shared.txt",s.replace("${id==='a'?'alpha':'omega'}","${id==='a'?'ALPHA':'OMEGA'}"));'`})}`)
    if(id==='c')calls.push(`TOOL local_run ${JSON.stringify({command:prompt.includes('PROJECT_CHAIN')?`node -e 'const fs=require("fs");fs.writeFileSync("shared.txt",fs.readFileSync("shared.txt","utf8").replace("ALPHA","FINAL"))'; cat shared.txt`:'cat shared.txt'})}`)
  }
  return { calls: correcting ? [] : calls, correcting, recovering, unified, delegated, imagePrivacy, videoPrivacy, generationPrompt }
}

function finalText(prompt, system, p, results, counts) {
  const { imageCount, documentCount } = counts
  const latestUserPrompt = prompt.startsWith('[Group chat ') && prompt.includes('\nUser: ') ? prompt.slice(prompt.lastIndexOf('\nUser: ') + 7) : prompt
  const round = prompt.match(/\[本轮群聊任务\]\n([\s\S]*?)\n\[\/本轮群聊任务\]/)?.[1]
  if (round && /MULTI_MENTION_CASE (\w+)/.test(round)) {
    const token = round.match(/MULTI_MENTION_CASE (\w+)/)[1]
    const field = key => round.match(new RegExp(`^${key}=(.*)$`, 'm'))?.[1]
    const prior = JSON.parse(field('已有回复')).length
    const handoff = token === 'followup' && prior < 2 ? (prior === 0 ? '\n@Beta 请补充核查。' : '\n@Alpha 请按核查结果修改。') : token === 'loop' ? (system.includes('你是 Alpha，') ? '\n@Beta 请再核查。' : '\n@Alpha 请再修改。') : ''
    return `MENTION_RESULT ${JSON.stringify({token, bot:field('当前执行者'), request:field('requestId'), pending:JSON.parse(field('尚未轮到的成员')), prior, errors:JSON.parse(field('失败记录')).length})}\n参与者包含 @Alpha 和 @Beta。${handoff}`
  }
  if (p.correcting) {
    const corrections = [...prompt.matchAll(/CORRECT_TO ([\w-]+)/g)].map(match => match[1])
    return `Corrected: ${corrections.at(-1) || 'updated'}; images=${imageCount} pdf=${documentCount}; original=${prompt.includes('ORIGINAL_EVIDENCE')}; extra=${prompt.includes('CORRECTION_EVIDENCE')}`
  }
  if (p.calls.length > 0 && !(prompt.includes('[技能试运行]') && !p.unified && !/TOOL /.test(prompt) && !prompt.match(/请先 read_skill 读取/))) {
    return p.imagePrivacy || p.videoPrivacy ? `${p.videoPrivacy ? 'video' : 'image'} prompt: ${p.generationPrompt}\n${results.join(' | ')}` : `Done: ${results.join(' | ')}`
  }
  if (p.recovering) return `Recovered from checkpoint; images=${imageCount} pdf=${documentCount}`
  if (/USE_EXPERIENCE ([\w-]+)/.test(prompt)) {
    const token=prompt.match(/USE_EXPERIENCE ([\w-]+)/)[1]
    const context=system.split('# 相关任务经验\n')[1]?.split('\n')[1]
    let learned=[];try{learned=JSON.parse(context)}catch{}
    return learned.some(l=>l.trigger.includes(token)&&l.method.includes('source records'))?`Applied experience: ${token}`:'No relevant experience'
  }
  if (/AUTO_SKILL_/.test(prompt)) {
    const prepared=system.split('# 本任务预备的技能\n')[1]
    let names=[]
    try { names=JSON.parse(prepared.split('\n')[1]).filter(s=>s.workflow?.includes('AUTO_SKILL_WORKFLOW')).map(s=>s.name) } catch {}
    return `Skills applied: ${names.join(', ') || 'none'}`
  }
  if (p.unified) {
    const token=prompt.match(/AUTO_WORKFLOW (\w+)/)?.[1], legacy=prompt.match(/AUTO_COORDINATE(?:_(?:FAIL|INVALID|SLOW))? (\w+)/)?.[1]
    if(token)return `Unified: ${token}; passed=${prompt.includes('"passed":false')?'false':'true'}`
    if(legacy)return prompt.includes('allocated-failure')?'Task failed: allocated-failure':`Integrated: ${legacy}`
    if(/\nFAIL ([^\n]+)/.test(prompt))return `Task failed: ${prompt.match(/\nFAIL ([^\n]+)/)[1]}`
    if(/AUTO_RESULT (\w+)/.test(prompt))return `Integrated: ${prompt.match(/AUTO_RESULT (\w+)/)[1]}`
    const rows=JSON.parse(prompt.match(/\n(\[\{"id":"task-[^\n]+\])/)[1]);return rows.map(t=>t.status==='failed'?`Task failed: ${t.result}`:t.result).filter(Boolean).join('\n')||'技能试运行结束。'
  }
  if(/^WORKFLOW_RESULT (\w+)$/m.test(prompt)) return `Workflow result ${prompt.match(/^WORKFLOW_RESULT (\w+)$/m)[1]}; upstream=${(prompt.match(/前置步骤结果 task-/g)||[]).length}`
  if(prompt.includes('[技能试运行]')) return '技能试运行产物已返回。'
  if (prompt.startsWith('[Reply from ') && /AUTO_RESULT (\w+)/.test(prompt)) return `Integrated: ${prompt.match(/AUTO_RESULT (\w+)/)[1]}`
  if (prompt.includes('[Automatically assigned task]') && /AUTO_RESULT (\w+)/.test(prompt)) return `AUTO_RESULT ${prompt.match(/AUTO_RESULT (\w+)/)[1]} verified${prompt.includes('<untrusted_attachment>') ? `; attachment=${prompt.includes('<untrusted_attachment>\nshared-evidence-marker')}` : ''}`
  if (p.delegated) return '已自动分配任务，等待协作结果。'
  if (/^QUEUE_ISOLATION (\w+)$/m.test(prompt)) return `Queue ${prompt.match(/^QUEUE_ISOLATION (\w+)$/m)[1]}: first=${prompt.includes('<untrusted_attachment>\nonly-first-attachment')}; second=${prompt.includes('<untrusted_attachment>\nonly-second-attachment')}`
  if (/^(?:[^:\n]+: )?REPORT_ATTACHMENT (.+)$/m.test(prompt)) {
    const needle = prompt.match(/^(?:[^:\n]+: )?REPORT_ATTACHMENT (.+)$/m)[1]
    return `Attachments: images=${imageCount} pdf=${documentCount}; text=${prompt.includes(`<untrusted_attachment>\n${needle}`)}`
  }
  if (/REPORT_GROUP_CONTEXT ([\w-]+)/.test(latestUserPrompt)) {
    const token = latestUserPrompt.match(/REPORT_GROUP_CONTEXT ([\w-]+)/)[1]
    return `Group context ${token}: ${prompt.includes(`CONTEXT_EVIDENCE ${token}`)}`
  }
  if (/你认领的是：CLAIMED_PART (\w+)/.test(prompt)) return `Claimed work: ${prompt.match(/你认领的是：CLAIMED_PART (\w+)/)[1]}`
  if (imageCount > 0) return `Saw ${imageCount} image(s): ${prompt.trim()}`
  if (/^ENV \S+$/m.test(prompt)) {
    const name = prompt.match(/^ENV (\S+)$/m)[1]
    return `${name}=${counts.envSet.includes(name) ? 'set' : 'unset'}`
  }
  const visiblePrompt = prompt.replace(/\n\n\[本轮群聊任务\]\n[\s\S]*?\n\[\/本轮群聊任务\]/g, '')
  return `Echo: ${visiblePrompt.trim().split('\n').at(-1)}`
}

export async function startFakeModel({ log } = {}) {
  const record = value => { if (log) appendFileSync(log, `${JSON.stringify(value)}\n`) }
  const server = createServer(async (request, response) => {
    // Decode across chunk boundaries: long Chinese requests can split a UTF-8 code point.
    request.setEncoding('utf8')
    let raw = ''
    for await (const chunk of request) raw += chunk
    if (request.method === 'GET' && request.url.endsWith('/models')) {
      response.writeHead(200, { 'content-type': 'application/json' })
      return response.end(JSON.stringify({ data: [{ id: 'fake-model' }] }))
    }
    const body = JSON.parse(raw || '{}')
    const json = (status, value) => { response.writeHead(status, { 'content-type': 'application/json' }); response.end(JSON.stringify(value)) }
    const refuse = (status, message) => json(status, { type: 'error', error: { type: 'invalid_request_error', message } })
    const system = typeof body.system === 'string' ? body.system : Array.isArray(body.system) ? body.system.map(b => b.text).join('\n\n') : ''
    if (body.tool_choice?.name === 'structured_answer') {
      const schema = body.tools[0].input_schema
      const text = blocksOf(body.messages[0]).map(b => b.text ?? '').join('')
      record({ args: ['--model', body.model, '--system-prompt', system, '--json-schema', JSON.stringify(schema), '--output-format', 'json', '--tools', ''], structuredInput: text })
      const { answer, delay } = structured(schema, text)
      if (delay) await sleep(delay)
      return json(200, { role: 'assistant', content: [{ type: 'tool_use', id: `toolu_${randomUUID().slice(0, 8)}`, name: 'structured_answer', input: answer }], stop_reason: 'tool_use', usage: { input_tokens: 100, output_tokens: 20, cache_read_input_tokens: 50 } })
    }
    const messages = body.messages ?? []
    const at = messages.findLastIndex(isPrompt)
    const promptBlocks = blocksOf(messages[at] ?? { content: [] })
    const counts = { envSet: (request.headers['x-dotbot-env'] ?? '').split(',').filter(Boolean), imageCount: promptBlocks.filter(b => b.type === 'image').length, documentCount: promptBlocks.filter(b => b.type === 'document').length }
    const prompt = promptBlocks.filter(b => b.type === 'text').map(b => b.text).join('\n').replace(/\n\n(?:\(Default language: [^\n]+\.\)|\[本轮表达要求\]\n[^\n]+)$/, '')
    const later = messages.slice(at + 1)
    const issued = later.filter(m => m.role === 'assistant').flatMap(blocksOf).filter(b => b.type === 'tool_use')
    const results = later.filter(m => m.role === 'user').flatMap(blocksOf).filter(b => b.type === 'tool_result').map(b => (Array.isArray(b.content) ? b.content : [{ text: b.content }]).map(c => c.text ?? '').join('\n'))
    const header = name => decodeURIComponent(request.headers[name] ?? '')
    const conversation = header('x-dotbot-conversation') || undefined
    const cwd = header('x-dotbot-cwd'), pluginDir = header('x-dotbot-plugin-dir'), envSet = header('x-dotbot-env').split(',').filter(Boolean)
    const resumed = header('x-dotbot-resume'), session = resumed || header('x-dotbot-session')
    const first = issued.length === 0
    if (first) {
      const args = ['--input-format', 'stream-json', '--model', body.model, '--system-prompt', system, '--mcp-config', header('x-dotbot-mcp') || '{"mcpServers":{}}']
      // What the request asked for in reasoning, so tests can see a default reach the model.
      if (body.thinking) args.push('--thinking', JSON.stringify(body.thinking))
      if (body.reasoning_effort) args.push('--reasoning-effort', String(body.reasoning_effort))
      if (resumed) args.push('--resume', resumed)
      else if (session) args.push('--session-id', session)
      if (pluginDir) args.push('--plugin-dir', pluginDir)
      record({ args, cwd })
      if (log) appendFileSync(`${log}.turns`, `${JSON.stringify({event:'start',pid:session,at:Date.now(),conversation,prompt})}\n`)
    }
    const end = () => { if (log) appendFileSync(`${log}.turns`, `${JSON.stringify({event:'end',pid:session,at:Date.now(),conversation})}\n`) }
    const p = plan(prompt, system, { cwd, pluginDir })
    if (first) {
      if (body.model === 'unsupported-model') { end(); return refuse(422, 'model "claude-opus-4-8" is not supported at this time') }
      if (prompt.includes('[工作流统一验收与交付]') && prompt.includes('SUMMARY_FAIL_ONCE') && log) {
        const marker = `${log}.summary-fail-once`
        if (!existsSync(marker)) { writeFileSync(marker, 'attempted'); end(); return refuse(400, 'fixture summary failed') }
      }
      const failOnce = prompt.match(/FAIL_ONCE ([\w-]+)/)
      if (failOnce && log) {
        const marker = `${log}.fail-once-${failOnce[1]}`
        if (!existsSync(marker)) { writeFileSync(marker, 'attempted'); end(); return refuse(400, 'fixture task failed once') }
      }
    }
    const transient = prompt.match(/^(?:[^:\n]+: )?TRANSIENT (once|always|after-tool) ([\w-]+)$/m)
    let transientFailure = false
    if (transient && log && (first || transient[1] === 'after-tool')) {
      const marker = `${log}.transient-${session || conversation}-${transient[2]}`
      transientFailure = transient[1] !== 'once' || !existsSync(marker)
      if (first) writeFileSync(marker, 'attempted')
    }
    const failTransient = () => {
      end()
      response.writeHead(200, { 'content-type': 'text/event-stream' })
      const send = event => response.write(`event: ${event.type}\ndata: ${JSON.stringify(event)}\n\n`)
      send({ type: 'message_start', message: { role: 'assistant', content: [], usage: { input_tokens: 1 } } })
      send({ type: 'content_block_start', index: 0, content_block: { type: 'text', text: '' } })
      send({ type: 'content_block_delta', index: 0, delta: { type: 'text_delta', text: '未完成的临时回复，不能作为完整结果。' } })
      send({ type: 'error', error: { type: 'overloaded_error', message: 'Server error mid-response. The response above may be incomplete.' } })
      response.end()
    }
    if (first && transientFailure && transient[1] !== 'after-tool') return failTransient()
    if (first) {
      // Group prompts prefix the first line of a message with its speaker.
      // Keep the synthetic task running there too, so correction tests can reach it.
      const delay = prompt.match(/^(?:[^:\n]+: )?DELAY (\d+)$/m)
      if (delay && !p.correcting && !p.unified) await sleep(Math.min(Number(delay[1]), 10000))
      const failure = prompt.match(/^(?:[^:\n]+: )?FAIL (.+)$/m)
      if (failure && !p.unified && !p.recovering) { end(); return refuse(400, failure[1]) }
      if (p.correcting && prompt.includes('CORRECTION_HOLD')) await sleep(1500)
    }
    const content = []
    if (issued.length < p.calls.length) {
      const [, name, args] = p.calls[issued.length].match(/^TOOL (\S+) ?(.*)$/)
      if (first && (p.imagePrivacy || p.videoPrivacy)) content.push({ type: 'text', text: `${p.videoPrivacy ? 'video' : 'image'} prompt: ${p.generationPrompt}` })
      const input = args ? JSON.parse(args) : {}
      if (name === 'local_send_file' && input.path?.startsWith('__TASK_CWD__/')) input.path = cwd + input.path.slice('__TASK_CWD__'.length)
      content.push({ type: 'tool_use', id: `toolu_${randomUUID().slice(0, 8)}`, name: name.startsWith('mcp__') || ['web_search', 'web_fetch', 'Skill'].includes(name) ? name : `mcp__teambot__${name}`, input })
    } else {
      if (transientFailure && transient[1] === 'after-tool') return failTransient()
      const afterTools = prompt.match(/^WAIT_AFTER_TOOLS (\d+)$/m)
      if (afterTools && !p.recovering) await sleep(Math.min(Number(afterTools[1]), 10000))
      content.push({ type: 'text', text: finalText(prompt, system, p, results, counts) })
      end()
    }
    const stop = content.at(-1).type === 'tool_use' ? 'tool_use' : 'end_turn'
    if (!body.stream) return json(200, { role: 'assistant', content, stop_reason: stop, usage: { input_tokens: 100, output_tokens: 20 } })
    response.writeHead(200, { 'content-type': 'text/event-stream' })
    const send = event => response.write(`event: ${event.type}\ndata: ${JSON.stringify(event)}\n\n`)
    send({ type: 'message_start', message: { role: 'assistant', content: [], usage: { input_tokens: 100, cache_read_input_tokens: 50 } } })
    content.forEach((block, index) => {
      if (block.type === 'text') {
        send({ type: 'content_block_start', index, content_block: { type: 'text', text: '' } })
        for (const piece of block.text.match(/.{1,12}/gs) ?? []) send({ type: 'content_block_delta', index, delta: { type: 'text_delta', text: piece } })
      } else {
        send({ type: 'content_block_start', index, content_block: { ...block, input: {} } })
        send({ type: 'content_block_delta', index, delta: { type: 'input_json_delta', partial_json: JSON.stringify(block.input) } })
      }
      send({ type: 'content_block_stop', index })
    })
    send({ type: 'message_delta', delta: { stop_reason: stop }, usage: { output_tokens: 20 } })
    send({ type: 'message_stop' })
    response.end()
  })
  server.listen(0, '127.0.0.1')
  await new Promise(resolve => server.once('listening', resolve))
  const url = `http://127.0.0.1:${server.address().port}`
  return { url, close: () => new Promise(resolve => { server.closeAllConnections?.(); server.close(() => resolve()) }) }
}
