# Regenerates src/mcp_store.json from ModelScope's MCP 广场 (https://www.modelscope.cn/mcp), a
# domestic marketplace with Chinese names and descriptions, install configs and view counts:
#
#   python3 -I dev/mcp-store-index.py src/mcp_store.json
#
# Candidates come from its public OpenAPI (every query returns at most the first 100 matches):
# one query per category plus a list of keywords for services people in China use. They are
# ranked by view count; the top ones are read in full for their install config. The store keeps
# the LIMIT most viewed servers that dotbot can actually run: a Chinese description, an npx/uvx
# command or a streamable-HTTP address, and no machine-specific paths. Nothing here talks to GitHub.
import concurrent.futures as cf
import json, re, sys, time, urllib.error, urllib.request
from datetime import date

OUT = sys.argv[1] if len(sys.argv) > 1 else 'src/mcp_store.json'
LIMIT = int(sys.argv[2]) if len(sys.argv) > 2 else 300
BASE = 'https://www.modelscope.cn'
LIST = BASE + '/openapi/v1/mcp/servers'
HEAD = {'content-type': 'application/json', 'user-agent': 'Mozilla/5.0 dotbot-store-index'}
CJK = re.compile(r'[一-鿿]')

# ModelScope's categories grouped under a few Chinese headings: [id, 中文, English].
GROUPS = [
    ('dev', '开发者工具', 'Developer tools'), ('search', '搜索与网页', 'Search & web'), ('browser', '浏览器自动化', 'Browser automation'),
    ('knowledge', '知识与记忆', 'Knowledge & memory'), ('data', '数据库与数据', 'Databases & data'), ('files', '文件与存储', 'Files & storage'),
    ('office', '办公与协作', 'Office & collaboration'), ('map', '地图与出行', 'Maps & travel'), ('finance', '金融与区块链', 'Finance & blockchain'),
    ('media', '图像音视频', 'Image, audio & video'), ('research', '研究与教育', 'Research & education'), ('ops', '云与运维', 'Cloud & operations'),
    ('social', '社交与电商', 'Social & commerce'), ('other', '其他', 'Other'),
]
OF = {}
for gid, ids in {
    'dev': 'developer-tools DeveloperTools code-analysis coding-agents code-execution shell-access command-line testing-and-qa-tools api-testing ci-cd software-architecture feature-flags agent-orchestration autonomous-agents',
    'search': 'search open-data government-data network-services',
    'browser': 'browser-automation',
    'knowledge': 'knowledge-and-memory Knowledge&Memory rag-systems vector-databases note-taking documentation-access text-summarization content-management-systems',
    'data': 'databases data-platforms data customer-data-platforms',
    'files': 'file-systems cloud-storage',
    'office': 'calendar-management communication productivity project-management customer-and-marketing customer-support erp-systems app-automation language-translation',
    'map': 'location-services travel-and-transportation travel transportation travel-services',
    'finance': 'finance blockchain cryptocurrency',
    'media': 'image-and-video-processing multimedia-processing audio-processing speech-processing text-to-speech AIGC art-and-culture entertainment-and-media games-and-gamification gaming',
    'research': 'research-and-data research-an-data Research&Data scientific-tool education education-and-learning-tools education-and-learning education-and-research biology-and-medicine bioinformatics health health-and-fitness health-and-wellness healthcare legal-and-compliance legal-and-government',
    'ops': 'cloud-platforms monitoring observability os-automation security security-and-iam security-and-compliance penetration-testing network-monitoring home-automation home-automation-and-iot energy',
    'social': 'social-media marketing ecommerce-and-retail real-estate sports sports-and-recreation fitness fitness-and-sports fitness-tracking',
}.items():
    for cid in ids.split(): OF[cid] = gid

# Services people in China use, so they are candidates even when a category's first 100 miss them.
KEYWORDS = '高德 百度 腾讯 阿里 飞书 钉钉 企业微信 微信 公众号 小红书 抖音 哔哩哔哩 知乎 淘宝 京东 支付宝 天气 新闻 股票 基金 翻译 OCR 12306 火车 语音 文生图 通义 智谱 DeepSeek 豆包 火山 硅基 Kimi 网易 美团 携程 招聘 法律 天眼查 联网搜索 网页抓取 数据库 文档 PDF Excel 邮件 日历 笔记 语雀 Notion GitHub 浏览器 文件系统 记忆 思维 时间 MySQL PostgreSQL Redis MongoDB SQLite Docker 代码 爬虫 图片 视频 音乐 地图 导航 快递 汇率 论文 PPT Word 表格 数据分析 可视化 流程图 绘图 二维码 短信 邮箱'.split()


def call(method, url, body=None, tries=5):
    for attempt in range(tries):
        try:
            request = urllib.request.Request(url, data=json.dumps(body).encode() if body is not None else None, method=method, headers=HEAD)
            with urllib.request.urlopen(request, timeout=60) as response:
                return json.load(response)
        except urllib.error.HTTPError as error:
            if error.code in (403, 404):
                return None  # quota or not found: not retried
            time.sleep(1 + attempt)
        except Exception:
            time.sleep(1 + attempt)
    return None


def query(filter_=None, search=''):
    answer = call('PUT', LIST, {'filter': filter_ or {}, 'page_number': 1, 'page_size': 100, 'search': search})
    return (answer or {}).get('data', {}).get('mcp_server_list', [])


def categories():
    answer = call('PUT', BASE + '/api/v1/dolphin/mcpServers', {'Filter': {}, 'PageNumber': 1, 'PageSize': 1, 'Search': ''})
    return [c['Value'] for c in answer['Data']['FiledAgg']['Category'] if c['Count'] >= 40]


def usable(detail):
    """The install settings dotbot can run, or None."""
    for item in detail.get('server_config') or []:
        servers = (item or {}).get('mcpServers') or {}
        for config in servers.values():
            if not isinstance(config, dict):
                continue
            command, args = config.get('command'), config.get('args') or []
            if command in ('npx', 'uvx') and all(isinstance(a, str) for a in args) and args:
                if any(re.search(r'(^|[\s=])(/Users/|/home/|[A-Za-z]:\\)', a) for a in args):
                    continue
                return {'kind': 'stdio', 'command': command, 'args': args, 'env': list((config.get('env') or {}).keys())}
            url = config.get('url')
            if isinstance(url, str) and url.startswith('https://') and 'modelscope' not in url and config.get('type') in ('streamable_http', 'streamableHttp', 'http'):
                return {'kind': 'http', 'url': url, 'env': list((config.get('headers') or {}).keys())}
    return None


def env_info(detail, names):
    props = ((detail.get('env_schema') or {}).get('properties') or {}) if isinstance(detail.get('env_schema'), dict) else {}
    required = set((detail.get('env_schema') or {}).get('required') or []) if isinstance(detail.get('env_schema'), dict) else set()
    out = []
    for name in names:
        p = props.get(name) or {}
        out.append({'name': name, 'desc': (p.get('description') or '').strip()[:200], 'required': name in required or not props})
    return out


def main():
    pool = {}
    jobs = [({'category': c}, '') for c in categories()] + [({}, k) for k in KEYWORDS]
    print('queries', len(jobs), flush=True)
    with cf.ThreadPoolExecutor(4) as pool_:
        for found in pool_.map(lambda j: query(*j), jobs):
            for s in found:
                pool[s['id']] = s
    ranked = sorted(pool.values(), key=lambda s: s.get('view_count') or 0, reverse=True)
    print('candidates', len(ranked), flush=True)
    chosen = [s for s in ranked if CJK.search((s.get('description') or '') + (s.get('chinese_name') or ''))]

    def read(summary):
        return summary, call('GET', LIST + '/' + urllib.request.quote(summary['id'], safe='@/'))

    servers, seen = [], set()

    def batches():
        # Details are read 90 at a time, most viewed first, until the store is full.
        with cf.ThreadPoolExecutor(6) as pool_:
            for start in range(0, len(chosen), 90):
                if len(servers) >= LIMIT:
                    return
                yield from pool_.map(read, chosen[start:start + 90])

    if True:
        for summary, answer in batches():
            detail = (answer or {}).get('data')
            if not detail or detail.get('is_hosted') and not detail.get('server_config'):
                continue
            how = usable(detail)
            description = (detail.get('description') or summary.get('description') or '').strip()
            if not how or not CJK.search(description):
                continue
            ids = [OF[c] for c in (summary.get('categories') or detail.get('categories') or []) if c in OF]
            servers.append({
                'id': summary['id'], 'name': (detail.get('chinese_name') or detail.get('name') or summary['id']).strip(),
                'desc': re.sub(r'\s+', ' ', description)[:300], 'cat': ids[0] if ids else 'other',
                'views': summary.get('view_count') or 0, 'author': detail.get('author') or '', 'verified': bool(detail.get('is_verified')),
                'stars': detail.get('github_stars') or 0, 'source': detail.get('source_url') or f"{BASE}/mcp/servers/{summary['id']}",
                'page': f"{BASE}/mcp/servers/{summary['id']}", 'logo': summary.get('logo_url') or '',
                **{k: v for k, v in how.items() if k != 'env'}, 'env': env_info(detail, how['env']),
            })
            # The same package listed twice keeps the most viewed entry.
            package = next((re.sub(r'@[\w.\-]+$', '', a) if not a.startswith('@') else re.sub(r'(?<=.)@[\w.\-]+$', '', a) for a in how.get('args', []) if not a.startswith('-')), how.get('url'))
            key = (how['kind'], how.get('command'), package)
            if key in seen:
                servers.pop()
                continue
            seen.add(key)
            if len(servers) >= LIMIT:
                break
    used = {s['cat'] for s in servers}
    out = {
        'source': 'ModelScope MCP 广场', 'url': BASE + '/mcp', 'fetched': date.today().isoformat(),
        'categories': [{'id': g, 'name': [zh, en]} for g, zh, en in GROUPS if g in used],
        'servers': servers,
    }
    with open(OUT, 'w', encoding='utf-8') as handle:
        json.dump(out, handle, ensure_ascii=False, separators=(',', ':'))
    print('wrote', OUT, len(servers), 'servers;', sorted(used))


main()
