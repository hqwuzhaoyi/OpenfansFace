"""Evidence resolution; folder hints cannot authorize skipping face analysis."""
import re
from pathlib import Path


def folder_hints(file, people):
    parts = Path(file).parts
    text = ' '.join(parts[-4:])
    output = []
    for name in sorted(set(people)):
        if len(name) < 2:
            continue
        pattern = re.escape(name)
        if name.isascii():
            pattern = r'(?<![A-Za-z0-9])' + pattern + r'(?![A-Za-z0-9])'
        if re.search(pattern, text, re.I):
            output.append(dict(name=name, source='folder', detail='文件名或文件夹名称命中'))
    return output[:10]


def candidates_from_text(watermark, mappings, folder):
    result = list(folder)
    for mark in watermark.get('candidates', []):
        mapping = mappings.get(mark['key'], {})
        if mapping.get('kind') == 'source' or (mark.get('platform') == 'Telegram' and mapping.get('kind') != 'person'):
            continue
        if mapping.get('kind') == 'person' and mapping.get('creator'):
            result.append(dict(name=mapping['creator'], source='watermark', confirmed=True,
                               key=mark['key'], detail=mark['text']))
        elif mark.get('kind') == 'account':
            token = mark['token'].lstrip('@').strip()
            if token and not re.search(r'[/\\\x00-\x1f]', token) and token not in ('.', '..') and len(token) <= 120:
                result.append(dict(name=token, source='watermark', confirmed=False,
                                   key=mark['key'], detail='账号尚未确认：' + mark['text']))
    return result


def can_skip_face(candidates, legacy_names=()):
    confirmed = {c['name'] for c in candidates if c.get('source') == 'watermark' and c.get('confirmed')}
    others = {c['name'] for c in candidates} | set(legacy_names)
    return len(confirmed) == 1 and others <= confirmed


def resolve_candidates(candidates):
    grouped = {}
    for candidate in candidates:
        name = candidate['name']
        entry = grouped.setdefault(name, dict(name=name, sources=[], evidence=[], score=None))
        if candidate['source'] not in entry['sources']:
            entry['sources'].append(candidate['source'])
        entry['evidence'].append(candidate)
        if candidate.get('score') is not None:
            entry['score'] = max(entry['score'] or 0, candidate['score'])
    values = sorted(grouped.values(), key=lambda c: (-('watermark' in c['sources']), -(c['score'] or 0), c['name']))
    return dict(candidates=values, kind='none' if not values else 'conflict' if len(values) > 1 else 'suggested',
                suggested=values[0]['name'] if len(values) == 1 else None)
