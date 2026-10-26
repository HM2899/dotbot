# Regenerates src/skill_store.json from a clone of https://github.com/anbeime/skill:
#   git clone https://github.com/anbeime/skill.git /tmp/skill && python3 -I dev/skill-store-index.py /tmp/skill src/skill_store.json
import json, re, subprocess, sys, unicodedata
repo, out = sys.argv[1], sys.argv[2]
commit = subprocess.check_output(['git', '-C', repo, 'rev-parse', 'HEAD'], text=True).strip()
rows = subprocess.check_output(['git', '-C', repo, 'ls-tree', '-r', '-l', 'HEAD', 'skills/'], text=True).splitlines()
files = []
for r in rows:
    meta, path = r.split('\t', 1)
    mode, kind, sha, size = meta.split()
    files.append((path, mode, kind, sha, int(size) if size != '-' else 0))
skill_dirs = sorted({p.rsplit('/', 1)[0] for p, *_ in files if p.endswith('/SKILL.md') and p.count('/') >= 2})
def clean(raw):
    n = ''.join(c if c.isascii() and c.isalnum() else '-' for c in raw.strip().lower())
    return '-'.join(x for x in n.split('-') if x)
def front(text):
    m = re.match(r'---\n(.*?)\n---', text.replace('\r\n', '\n'), re.S)
    if not m: return '', ''
    name = desc = ''
    lines = m.group(1).split('\n')
    for i, l in enumerate(lines):
        if l.startswith('name:'): name = l[5:].strip().strip('"\'')
        elif l.startswith('description:'):
            v = l[12:].strip()
            if v in ('>', '>-', '|', '|-'):
                body = []
                for x in lines[i+1:]:
                    if x.startswith((' ', '\t')): body.append(x.strip())
                    else: break
                v = ' '.join(body)
            desc = v.strip().strip('"\'')
    return name, desc
CATEGORY = [
    ('video', 'Video', ['video', 'infinitetalk', 'remotion', 'avatar', 'dream-video', 'transcript', 'srt', 'kb-to-mpt', 'media-processor']),
    ('commerce', 'Commerce & marketing', ['commerce', 'shopping', 'copywriter', 'xiaohongshu', 'pet-', 'marketing']),
    ('content', 'Content & publishing', ['content', 'article', 'baoyu', 'wechat', 'publisher', 'moltbook', 'hotspot', 'historical', 'poetry', 'xhs', 'bedtime']),
    ('slides', 'Presentations', ['ppt', 'nanobanana']),
    ('audio', 'Voice & audio', ['tts', 'asr']),
    ('documents', 'Documents & research', ['doc', 'pdf', 'markdown', 'paper', 'law', 'contract', 'obsidian', 'json-canvas', 'four-color', 'provenance', 'data-storytelling', 'stock', 'resume', 'convert']),
    ('design', 'Design & visuals', ['design', 'icon', 'illustration', 'image', 'logo', 'drawio', 'archify', 'photo', 'cover', 'threejs']),
    ('dev', 'Development & automation', ['chrome', 'web-to-app', 'security', 'ontoly', 'agent', 'meeting', 'email', 'product-manager']),
]
def category(name):
    for key, _, words in CATEGORY:
        if any(w in name for w in words): return key
    return 'other'
COMMON = {'content-research-writer', 'convert-documents-to-markdown', 'data-storytelling'}
SKIP_NAMES = {'frontend-design', 'skill-template'}  # covered by anthropic/frontend-design; a template, not a skill
entries, rejected = {}, []
for d in skill_dirs:
    inside = [f for f in files if f[0].startswith(d + '/')]
    # Hidden files (.gitignore and the like) are not skill content and dotbot refuses dot paths.
    inside = [f for f in inside if not any(part.startswith('.') for part in f[0][len(d)+1:].split('/'))]
    nested = any(f[0].endswith('/SKILL.md') and f[0] != d + '/SKILL.md' for f in inside)
    skill = subprocess.check_output(['git', '-C', repo, 'show', f'HEAD:{d}/SKILL.md']).decode('utf-8', 'replace')
    title, desc = front(skill)
    name = clean(title or d.rsplit('/', 1)[1])
    why = None
    if not desc: why = 'no description'
    elif name in SKIP_NAMES: why = 'skipped'
    elif len(name) > 48: why = 'name too long'
    elif any(m not in ('100644', '100755') or k != 'blob' for _, m, k, _, _ in inside): why = 'links or submodules'
    elif len(inside) > 200: why = f'{len(inside)} files'
    elif any(s > 2_000_000 for *_, s in inside): why = 'a file over 2 MB'
    elif sum(s for *_, s in inside) > 16_000_000: why = 'over 16 MB'
    elif len(skill.encode()) > 128_000: why = 'SKILL.md too large'
    if why: rejected.append(f'{d}: {why}'); continue
    rank = (nested, not d.startswith('skills/'), d != f'skills/{d.split("/")[1]}/{d.rsplit("/",1)[1]}', len(d))
    entry = {'name': name, 'title': title or name, 'description': unicodedata.normalize('NFC', desc)[:400], 'category': category(name),
             'path': d, 'files': [[f[0][len(d)+1:], f[3], f[4]] for f in inside], 'common': name in COMMON, '_rank': rank}
    if name not in entries or rank < entries[name]['_rank']: entries[name] = entry
for e in entries.values(): e.pop('_rank')
data = {'repository': 'anbeime/skill', 'commit': commit, 'license': 'See the upstream repository: its README lists CC BY 4.0 and MIT; individual skills may carry their own terms.',
        'categories': [[k, n] for k, n, _ in CATEGORY] + [['other', 'Other']], 'skills': sorted(entries.values(), key=lambda e: e['name'])}
json.dump(data, open(out, 'w', encoding='utf-8'), ensure_ascii=False, separators=(',', ':'))
print(len(entries), 'skills;', 'rejected:', *rejected, sep='\n  ')
from collections import Counter; print(Counter(e['category'] for e in entries.values()))
