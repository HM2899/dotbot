// Plugin catalog data: what each kind of MCP connection and skill is for, and the MCP store.
// Texts are [中文, English]; pick() chooses by the page language (LANG is defined in i18n.js).
const pick = pair => (typeof LANG !== 'undefined' && LANG === 'zh' ? pair[0] : pair[1])

// MCP = a tool server Bots call (read files, query a database, search the web…).
// Skill = a written workflow Bots follow (steps, checks, deliverables). They are different things.
const MCP_CATEGORIES = {
  dev: ['开发者工具', 'Developer tools', '代码仓库、技术文档、测试与调试：让 Bot 读代码、查文档、处理工单和线上问题。', 'Repositories, docs, testing and debugging: Bots read code, look up docs and handle issues.'],
  search: ['搜索与网页', 'Search & web', '联网搜索、网页抓取，给 Bot 补充实时信息。', 'Web search and page fetching so Bots have fresh information.'],
  browser: ['浏览器自动化', 'Browser automation', '让 Bot 打开网页、点击、填表、截图，完成必须在浏览器里做的事。', 'Bots open pages, click, fill forms and take screenshots.'],
  knowledge: ['知识与记忆', 'Knowledge & memory', '长期记忆、笔记、知识库和文档检索，帮 Bot 处理跨多轮的复杂任务。', 'Long-term memory, notes and knowledge bases for complex, multi-turn work.'],
  data: ['数据库与数据', 'Databases & data', '查询和分析本地或远程数据库里的数据。', 'Query and analyse data in local or remote databases.'],
  files: ['文件与存储', 'Files & storage', '读写你授权目录里的文件，对接云存储。', 'Read and write files in folders you allow, and cloud storage.'],
  office: ['办公与协作', 'Office & collaboration', '日历、邮件、文档、聊天和项目管理：让 Bot 使用团队日常的办公工具。', 'Calendar, mail, documents, chat and project tools your team already uses.'],
  map: ['地图与出行', 'Maps & travel', '地图、路线、天气、车票和航班查询。', 'Maps, routes, weather, train tickets and flights.'],
  finance: ['金融与区块链', 'Finance & blockchain', '行情、支付和链上数据查询。', 'Quotes, payments and on-chain data.'],
  media: ['图像音视频', 'Image, audio & video', '图片、音频、视频的生成、识别和处理，以及图表、思维导图等可视化。', 'Generate, recognise and process images, audio and video, plus charts and mind maps.'],
  research: ['研究与教育', 'Research & education', '论文检索、科研数据、学习和健康医疗资料。', 'Papers, research data, learning and health references.'],
  ops: ['云与运维', 'Cloud & operations', '云平台、监控、安全和系统自动化。', 'Cloud platforms, monitoring, security and system automation.'],
  social: ['社交与电商', 'Social & commerce', '社交媒体、内容运营和电商数据。', 'Social media, content operations and e-commerce data.'],
  other: ['其他', 'Other', '暂未归入以上类别的工具。', 'Tools that fit no other category.'],
  custom: ['自定义', 'Custom', '你手动添加的连接，用途由你自己决定。', 'Connections you added by hand; their purpose is up to you.'],
}

// Skill kinds follow the skill store's categories, plus `custom` for skills written here or by Bots.
const SKILL_CATEGORIES = {
  official: ['官方', 'Official', '随系统内置的基础工作流。', 'Core workflows that ship with the system.'],
  video: ['视频', 'Video', '脚本、分镜、剪辑与成片流程，适合做短视频和动态漫。', 'Scripts, storyboards, editing and delivery for short video and animated stories.'],
  commerce: ['电商与营销', 'Commerce & marketing', '选品、文案、投放与运营分析，服务电商与增长工作。', 'Product copy, campaigns and operations analysis for commerce and growth.'],
  content: ['内容与发布', 'Content & publishing', '写作、排版与多平台发布，覆盖公众号、社交媒体等内容生产。', 'Writing, layout and publishing across articles and social platforms.'],
  slides: ['演示文稿', 'Presentations', '生成和美化 PPT、讲稿与汇报材料。', 'Create and polish slide decks, speaker notes and reports.'],
  audio: ['语音与音频', 'Voice & audio', '配音、转写、音频处理。', 'Voice-over, transcription and audio processing.'],
  documents: ['文档与研究', 'Documents & research', '资料调研、文档整理、报告撰写与数据归纳。', 'Research, document handling, report writing and summaries.'],
  design: ['设计与视觉', 'Design & visuals', '海报、图表、界面与品牌视觉的设计流程。', 'Posters, charts, interfaces and brand visuals.'],
  dev: ['开发与自动化', 'Development & automation', '编码、测试、部署与多 Bot 协作等工程类工作流。', 'Engineering workflows: coding, testing, deployment and multi-Bot teamwork.'],
  other: ['其他', 'Other', '暂未归入以上类别的专项技能。', 'Specialist skills that fit no other category.'],
  custom: ['自建', 'Custom', '你或 Bot 在这里编写、导入的工作流，尚未经过官方筛选，首次使用前建议试运行。', 'Workflows written or imported here by you or a Bot. Not curated: try them before relying on them.'],
}

// What each sign-in service gives Bots, shown on its card under Connections & keys.
const ACCOUNT_USE = {
  google: ['Gmail、日历、云端硬盘、文档、表格、通讯录、任务和相册，每项服务可单独设为关闭、只读或读写。', 'Gmail, Calendar, Drive, Docs, Sheets, Contacts, Tasks and Photos, each set to off, read or read & write.'],
  github: ['读取你的 GitHub 资料与仓库；开启写入后可管理仓库和 Webhook。', 'Reads your GitHub profile and repositories; with writes on, manages repositories and webhooks.'],
  microsoft: ['Outlook 邮件、日历和 OneDrive 文件，可开启发信与编辑。', 'Outlook mail, calendar and OneDrive files, with optional sending and editing.'],
}

// Servers saved from the earlier hand-picked store still get their name and icon: this list is no longer shown in the store.
// match: text found in a saved connection's command or address, so old and manual connections get the same icon and description.
// secrets: environment variables the server needs; the add dialog saves them as keys and refers to them as ${NAME}.
// param: one value that goes after the command (a folder or file); header: a remote server's Authorization key.
const MCP_STORE = [
  { id: 'github', category: 'dev', icon: 'github', color: 'var(--text)', match: ['server-github', 'github-mcp'], kind: 'stdio', command: 'npx', args: ['-y', '@modelcontextprotocol/server-github'], secrets: ['GITHUB_PERSONAL_ACCESS_TOKEN'], docs: 'https://github.com/modelcontextprotocol/servers/tree/main/src/github',
    name: 'GitHub', use: ['读取和搜索仓库、Issue、Pull Request，创建 Issue、提交评论。需要 GitHub 访问令牌。', 'Browse and search repositories, issues and pull requests; open issues and comment. Needs a GitHub token.'] },
  { id: 'context7', category: 'dev', mark: 'C7', color: '#059669', match: ['context7'], kind: 'stdio', command: 'npx', args: ['-y', '@upstash/context7-mcp'], secrets: [], docs: 'https://github.com/upstash/context7',
    name: 'Context7', use: ['按库的最新版本查询官方文档和代码示例，避免 Bot 用过时的接口写代码。', 'Looks up current official docs and examples for a library version so Bots do not code against outdated APIs.'] },
  { id: 'sentry', category: 'dev', mark: 'S', color: '#6c5fc7', match: ['mcp.sentry.dev'], kind: 'http', url: 'https://mcp.sentry.dev/mcp', secrets: [], docs: 'https://docs.sentry.io/product/sentry-mcp/',
    name: 'Sentry', use: ['查看线上错误、堆栈和影响范围，协助定位问题。首次使用需在浏览器完成 Sentry 授权。', 'Read production errors, stack traces and impact to help diagnose problems. First use needs Sentry authorization in a browser.'] },
  { id: 'filesystem', category: 'files', mark: 'Fs', color: '#d97706', match: ['server-filesystem'], kind: 'stdio', command: 'npx', args: ['-y', '@modelcontextprotocol/server-filesystem'], secrets: [], docs: 'https://github.com/modelcontextprotocol/servers/tree/main/src/filesystem',
    param: { label: ['允许访问的目录', 'Folder Bots may access'], value: '/config/workspace' },
    name: 'Filesystem', use: ['让 Bot 读取、搜索、编辑你指定目录里的文件。只能访问该目录，不会碰其他位置。', 'Lets Bots read, search and edit files inside one folder you choose, and nowhere else.'] },
  { id: 'time', category: 'files', mark: '⏱', color: '#0ea5e9', match: ['mcp-server-time'], kind: 'stdio', command: 'uvx', args: ['mcp-server-time'], secrets: [], docs: 'https://github.com/modelcontextprotocol/servers/tree/main/src/time',
    name: 'Time', use: ['查询当前时间并在不同时区之间换算，适合安排跨时区的日程。需要本机装有 uv。', 'Current time and time-zone conversion, handy for scheduling across regions. Needs uv installed.'] },
  { id: 'fetch', category: 'search', mark: '↓', color: '#2563eb', match: ['mcp-server-fetch'], kind: 'stdio', command: 'uvx', args: ['mcp-server-fetch'], secrets: [], docs: 'https://github.com/modelcontextprotocol/servers/tree/main/src/fetch',
    name: 'Fetch', use: ['抓取网页并转成易读的文本，用来阅读文章、文档页面。不执行页面脚本。需要本机装有 uv。', 'Fetches a web page as readable text for articles and docs. Does not run page scripts. Needs uv installed.'] },
  { id: 'brave-search', category: 'search', mark: 'B', color: '#fb542b', match: ['server-brave-search'], kind: 'stdio', command: 'npx', args: ['-y', '@modelcontextprotocol/server-brave-search'], secrets: ['BRAVE_API_KEY'], docs: 'https://brave.com/search/api/',
    name: 'Brave Search', use: ['联网搜索网页和新闻，给 Bot 提供实时信息。需要 Brave Search API Key。', 'Web and news search for up-to-date information. Needs a Brave Search API key.'] },
  { id: 'sqlite', category: 'data', mark: 'SQ', color: '#0f80cc', match: ['mcp-server-sqlite'], kind: 'stdio', command: 'uvx', args: ['mcp-server-sqlite', '--db-path'], secrets: [], docs: 'https://github.com/modelcontextprotocol/servers/tree/main/src/sqlite',
    param: { label: ['SQLite 数据库文件路径', 'SQLite database file'], value: '/config/workspace/data.db' },
    name: 'SQLite', use: ['查询和分析一个 SQLite 数据库文件，可做数据汇总与报表。需要本机装有 uv。', 'Query and analyse one SQLite database file for summaries and reports. Needs uv installed.'] },
  { id: 'notion', category: 'office', mark: 'N', color: '#111111', match: ['notion-mcp', 'notionhq'], kind: 'stdio', command: 'npx', args: ['-y', '@notionhq/notion-mcp-server'], secrets: ['NOTION_TOKEN'], docs: 'https://github.com/makenotion/notion-mcp-server',
    name: 'Notion', use: ['搜索、读取和更新 Notion 页面与数据库，需要先把页面分享给对应的集成。', 'Search, read and update Notion pages and databases. Share the pages with your integration first.'] },
  { id: 'slack', category: 'office', mark: '#', color: '#4a154b', match: ['server-slack'], kind: 'stdio', command: 'npx', args: ['-y', '@modelcontextprotocol/server-slack'], secrets: ['SLACK_BOT_TOKEN', 'SLACK_TEAM_ID'], docs: 'https://github.com/modelcontextprotocol/servers/tree/main/src/slack',
    name: 'Slack', use: ['读取频道消息、发送消息、回复话题。需要 Slack Bot 令牌和工作区 ID。', 'Read channels, post messages and reply in threads. Needs a Slack bot token and workspace ID.'] },
  { id: 'linear', category: 'office', mark: 'L', color: '#5e6ad2', match: ['mcp.linear.app'], kind: 'http', url: 'https://mcp.linear.app/mcp', secrets: [], docs: 'https://linear.app/docs/mcp',
    name: 'Linear', use: ['查看、创建和更新 Linear 的任务与项目。首次使用需在浏览器完成 Linear 授权。', 'View, create and update Linear issues and projects. First use needs Linear authorization in a browser.'] },
  { id: 'memory', category: 'knowledge', mark: 'M', color: '#7c3aed', match: ['server-memory'], kind: 'stdio', command: 'npx', args: ['-y', '@modelcontextprotocol/server-memory'], secrets: [], docs: 'https://github.com/modelcontextprotocol/servers/tree/main/src/memory',
    name: 'Memory', use: ['用知识图谱保存人物、项目、偏好等长期信息，跨对话可检索。与 Bot 自带的记忆互补。', 'A knowledge graph for people, projects and preferences that persists across chats. Complements the Bots’ built-in memory.'] },
  { id: 'sequential-thinking', category: 'knowledge', mark: '1·2', color: '#db2777', match: ['sequential-thinking'], kind: 'stdio', command: 'npx', args: ['-y', '@modelcontextprotocol/server-sequential-thinking'], secrets: [], docs: 'https://github.com/modelcontextprotocol/servers/tree/main/src/sequentialthinking',
    name: 'Sequential Thinking', use: ['让 Bot 把复杂问题拆成步骤，边想边修正，适合规划和排错。', 'Helps Bots break a hard problem into steps and revise as they go; good for planning and debugging.'] },
]

// What a server or saved connection runs, as one comparable key: kind, command and package without its version.
function mcpKeyOf(kind, command, args, url) {
  if (kind === 'http') return `http||${String(url ?? '').replace(/\/+$/, '')}`
  const pkg = (args ?? []).find(a => typeof a === 'string' && !a.startsWith('-')) ?? ''
  return `stdio|${command}|${pkg.replace(/(?<=.)@[\w.\-]+$/, '')}`
}
const mcpServerKey = s => mcpKeyOf(s.kind, s.command, s.args, s.url)
const mcpConnectionKey = c => mcpKeyOf(c.kind, c.command, c.args, c.url)
const MARK_PALETTE = ['#13b99a', '#ff8946', '#578bea', '#ef658d', '#9c79db', '#d7a25a', '#e85c65', '#36b8c9']
// A store server shaped like a catalog entry, for cards of connections added from the store.
function mcpServerEntry(server) {
  const hash = [...server.name].reduce((n, c) => (n * 31 + c.codePointAt(0)) >>> 0, 7)
  return { id: `store:${server.id}`, category: server.cat, name: server.name, use: [server.desc, server.desc], mark: [...server.name][0]?.toUpperCase() ?? '?', color: MARK_PALETTE[hash % MARK_PALETTE.length], docs: server.page }
}

// The catalog entry a saved connection corresponds to, by name or by what it runs.
function mcpEntryFor(connection) {
  const text = [connection.command, ...(connection.args ?? []), connection.url].join(' ').toLowerCase()
  const known = MCP_STORE.find(e => e.id === connection.name) ?? MCP_STORE.find(e => e.match.some(m => text.includes(m)))
  if (known) return known
  const key = mcpConnectionKey(connection), server = (typeof mcpDirectory !== 'undefined' ? mcpDirectory?.servers : null)?.find(x => mcpServerKey(x) === key)
  return server ? mcpServerEntry(server) : null
}
