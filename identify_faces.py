import sqlite3, numpy as np, json, time, os
from collections import defaultdict, Counter

BASE = os.environ.get('FACES_BASE', '/data/base')
OUT = os.environ.get('F_OUT', BASE)     # 输出重定向（dry-run 用），默认与 BASE 相同
from tplgroups import mutual_groups

OVERRIDES = {}
try:
    OVERRIDES = json.load(open(f'{BASE}/template-overrides.json'))
except Exception:
    pass
T_EDGE, K, TAU_C = 0.47, 4, 0.50   # 脸聚类参数（与 v2 一致）
T_CLAIM = float(os.environ.get('F_TCLAIM', '0.50'))
T_STRONG = float(os.environ.get('F_TSTRONG', '0.58'))
MULTI = bool(os.environ.get('F_MULTI'))   # 多质心注册表（每身份按形态建多模板，max-pool 计分）
MULTI_EX = [s for s in os.environ.get('F_MULTI_EXCLUDE', '').split(',') if s.strip()]
_ex_key = lambda s_: ''.join(ch for ch in s_ if ch.isalnum()).lower()
MULTI_EXK = {_ex_key(s) for s in MULTI_EX if _ex_key(s)}
# F_MULTI_EXCLUDE: 逗号分隔身份名（按去符号归一化匹配）——模板污染的身份保持单质心
MIN_TPL = 5                        # 注册模板至少 5 张脸

def rel(vid):
    w = vid.replace('\\', '/')
    if '/pikpak/' in w:
        w = w.split('/pikpak/', 1)[1]
    return w

def identity_of(p):
    """返回 (人物名, 来源类型)。None = 未分类。"""
    if p[0] == 'creators' and len(p) >= 3:
        return p[1], 'creators'
    if p[0] == 'share':
        if len(p) >= 3 and p[1] == '收藏':
            return p[2], 'share'
        if len(p) >= 2:
            return p[1], 'share'
    if p[0] == 'My Telegram' and len(p) >= 3 and p[1].startswith('#'):
        return p[1].split()[0], 'tag'
    return None, None

t0 = time.time()
db = sqlite3.connect(f'{BASE}/faces.db')
vids = {}
for vid, path, name, size, dur, zone, coll in db.execute(
        "SELECT vid,path,name,size,duration,zone,collection FROM videos WHERE status='done'").fetchall():
    p = rel(path).split('/')
    ident, kind = identity_of(p)
    vids[vid] = {'name': name, 'size': size, 'duration': dur, 'thumb': None,
                 'ident': ident, 'kind': kind, 'unsorted': ident is None}

rows = db.execute("""SELECT f.vid, f.score, f.emb, f.thumb FROM faces f
                     JOIN videos v ON v.vid=f.vid WHERE v.status='done' ORDER BY f.id""").fetchall()
fvid = [r[0] for r in rows]
fsc = [r[1] for r in rows]
fth = [r[2] for r in rows]  # placeholder, real thumb below
E = np.stack([np.frombuffer(r[2], np.float32) for r in rows])
E /= np.linalg.norm(E, axis=1, keepdims=True) + 1e-9
fthumb = [r[3] for r in rows]
N = len(E)
for r in rows:
    if r[0] in vids:
        vids[r[0]]['thumb'] = r[3]
print(f"faces={N} videos={len(vids)} unsorted={sum(1 for v in vids.values() if v['unsorted'])}")

def cluster_embeddings(Esub, thresh=T_EDGE, k=K, tau=TAU_C):
    """互近邻(k) + 质心二次合并，返回 labels 数组"""
    M = len(Esub)
    if M == 0:
        return np.zeros(0, np.int32)
    tidx = np.full((M, k), -1, np.int32)
    tsim = np.zeros((M, k), np.float32)
    kk = min(k, M-1)
    for i0 in range(0, M, 2048):
        S = Esub[i0:i0+2048] @ Esub.T
        for r in range(S.shape[0]):
            i = i0 + r
            S[r, i] = -1.0
            ii = np.argpartition(-S[r], kk-1)[:kk]
            tidx[i, :kk] = ii
            tsim[i, :kk] = S[r, ii]
    parent = list(range(M))
    def find(x):
        while parent[x] != x:
            parent[x] = parent[parent[x]]
            x = parent[x]
        return x
    for i in range(M):
        for jx in range(kk):
            j = int(tidx[i, jx])
            if j < 0 or tsim[i, jx] < thresh:
                continue
            rev = np.where(tidx[j] == i)[0]
            if len(rev) and tsim[j, rev[0]] >= thresh:
                ra, rb = find(i), find(j)
                if ra != rb:
                    parent[ra] = rb
    grp = defaultdict(list)
    for i in range(M):
        grp[find(i)].append(i)
    cents = {}
    for r_, ixs in grp.items():
        c = Esub[ixs].mean(0)
        cents[r_] = c / (np.linalg.norm(c) + 1e-9)
    roots = sorted(cents, key=lambda r_: -len(grp[r_]))
    for a in range(len(roots)):
        for b in range(a+1, len(roots)):
            ra, rb = find(roots[a]), find(roots[b])
            if ra != rb and float(cents[ra] @ cents[rb]) >= tau:
                parent[ra] = rb
    lab = np.zeros(M, np.int32)
    remap = {}
    for i in range(M):
        r_ = find(i)
        if r_ not in remap:
            remap[r_] = len(remap)
        lab[i] = remap[r_]
    return lab

# ---- 全局脸聚类（一次），供注册模板挑主脸堆 ----
labels = cluster_embeddings(E)
print(f"global face clusters={len(set(labels))} ({time.time()-t0:.0f}s)")

# ---- 构建注册库：每个人物文件夹 → 主脸堆模板 ----
id_faces = defaultdict(list)      # ident -> face idx list
for i in range(N):
    v = vids[fvid[i]]
    if v['ident']:
        id_faces[v['ident']].append(i)

registry = []   # {name, kind, tpl_idx(face idxs), cent}

def _name_key(s):
    return ''.join(ch for ch in s if ch.isalnum()).lower()

for ident, idxs in id_faces.items():
    tpl = None
    ov = OVERRIDES.get(ident)
    if ov and ov.get('groups') is not None:
        gs = mutual_groups(E[idxs])
        cand = [idxs[i] for g in ov['groups'] if g < len(gs) for i in gs[g]]
        if len(cand) >= MIN_TPL:
            tpl = cand
    key = _name_key(ident)[:4]
    if len(key) >= 2:
        nm = [i for i in idxs if key in fvid[i].lower()]
        if MIN_TPL <= len(nm) < len(idxs):
            # 文件名含本名的子集 → 局部聚类取最大组，防文件夹混片污染模板
            lab_loc = cluster_embeddings(E[nm])
            top_lab, top_n = Counter(lab_loc).most_common(1)[0]
            cand = [nm[i] for i in range(len(nm)) if lab_loc[i] == top_lab]
            if len(cand) >= MIN_TPL:
                tpl = cand
    if tpl is None:
        lab_cnt = Counter(labels[i] for i in idxs)
        top_lab, top_n = lab_cnt.most_common(1)[0]
        if top_n < MIN_TPL:
            continue
        tpl = [i for i in idxs if labels[i] == top_lab]
    c = E[tpl].mean(0)
    c /= np.linalg.norm(c) + 1e-9
    kind = vids[fvid[idxs[0]]]['kind']
    tvecs_ = None
    if MULTI and _ex_key(ident) not in MULTI_EXK:
        lt_ = cluster_embeddings(E[tpl])
        gs_ = sorted([[tpl[i] for i in range(len(tpl)) if lt_[i] == g] for g in set(lt_)],
                     key=len, reverse=True)
        cs_ = [E[g].mean(0) for g in gs_ if len(g) >= 3]
        cs_ = [x / (np.linalg.norm(x) + 1e-9) for x in cs_]
        if len(cs_) >= 2:   # 去重：余弦>0.92 的向量视为同模板
            ded_ = [cs_[0]]
            for v_ in cs_[1:]:
                if max(float(v_ @ d_) for d_ in ded_) < 0.92:
                    ded_.append(v_)
            cs_ = ded_
        tvecs_ = np.stack(cs_).astype(np.float32) if cs_ else None
    registry.append({'name': ident, 'kind': kind, 'tpl': tpl, 'cent': c, 'tvecs': tvecs_,
                     'folder_n': len({fvid[i] for i in idxs}),
                     'folder_gb': sum(vids[fvid[i]]['size'] for i in idxs)/2**30})
print(f"registry identities={len(registry)}")

# ---- 合并同脸不同名（娜娜/台北娜娜）：用户规则 2026-10-30——名字不同不合并，仅同名文件夹并组 ----
import re as _re
def _sq(n):
    return _re.sub(r'[\s\-_·（）()#]+', '', n).lower()
par = list(range(len(registry)))
def find(x):
    while par[x] != x:
        par[x] = par[par[x]]
        x = par[x]
    return x
for a in range(len(registry)):
    for b in range(a+1, len(registry)):
        ra, rb = find(a), find(b)
        if (ra != rb and _sq(registry[a]['name']) == _sq(registry[b]['name'])
                and float(registry[a]['cent'] @ registry[b]['cent']) >= 0.55):
            par[ra] = rb
merged_reg = defaultdict(list)
for i in range(len(registry)):
    merged_reg[find(i)].append(i)
registry2 = []
for r_, members in merged_reg.items():
    members.sort(key=lambda i: -len(registry[i]['tpl']))
    main = registry[members[0]]
    tpl = sorted({i for m in members for i in registry[m]['tpl']})
    c = E[tpl].mean(0)
    c /= np.linalg.norm(c) + 1e-9
    if MULTI:
        tv_list = [registry[m]['tvecs'] for m in members]
        # 合并组任一成员被拉黑（tvecs None）→ 整组保持合并质心（=生产行为）
        tvecs_m = np.vstack(tv_list) if all(t is not None for t in tv_list) else np.stack([c])
    else:
        tvecs_m = None
    registry2.append({'name': main['name'],
                      'aliases': [registry[m]['name'] for m in members[1:]],
                      'kind': main['kind'], 'tpl': tpl, 'cent': c,
                      'tvecs': tvecs_m,
                      'folder_n': sum(registry[m]['folder_n'] for m in members),
                      'folder_gb': sum(registry[m]['folder_gb'] for m in members)})
print(f"after alias-merge: {len(registry2)} identities")

# ---- 1:N 认领未分类视频 ----
C = np.stack([r['cent'] for r in registry2]) if registry2 else np.zeros((0, 512), np.float32)
if MULTI and registry2 and all(r['tvecs'] is not None for r in registry2):
    TV = np.vstack([r['tvecs'] for r in registry2]).astype(np.float32)
    TOWN = np.repeat(np.arange(len(registry2)), [len(r['tvecs']) for r in registry2])
    n_mv = len(TV)
    print(f"multi-centroid: {n_mv} template vectors for {len(registry2)} identities "
          f"(max {len(TV)//max(len(registry2),1)})")
else:
    TV = TOWN = None
    n_mv = 0
unsorted_face = [i for i in range(N) if vids[fvid[i]]['unsorted']]
claims = defaultdict(Counter)   # vid -> Counter(id_idx -> n_faces)
best_sim = defaultdict(float)
if len(C):
    if TV is not None:
        AV = E[unsorted_face] @ TV.T            # (nf, n_vectors)
        UF = np.zeros((len(unsorted_face), len(registry2)), np.float32)
        for j_ in range(len(registry2)):
            cols_ = AV[:, TOWN == j_]
            if cols_.shape[1]:
                UF[:, j_] = cols_.max(axis=1)
    else:
        UF = E[unsorted_face] @ C.T             # (n_unsorted_faces, n_ids)
    for fi, i_vid in enumerate(unsorted_face):
        vid = fvid[i_vid]
        row = UF[fi]
        for idj in np.where(row >= T_CLAIM)[0]:
            claims[vid][int(idj)] += 1
            best_sim[(vid, int(idj))] = max(best_sim[(vid, int(idj))], float(row[idj]))

# ---- 在位认领保护（仅 F_MULTI：对既有分配只增不翻，除决定性差距）----
INC = {}
inc_dropped = inc_missing = 0
if MULTI and os.path.exists(f'{BASE}/report.json'):
    def _jx(nm_):
        for k_, r__ in enumerate(registry2):
            if r__['name'] == nm_ or nm_ in (r__['aliases'] or []):
                return k_
        return None
    for it_ in json.load(open(f'{BASE}/report.json'))['identities']:
        jx_ = _jx(it_['name'])
        if jx_ is None:
            continue
        for v_ in it_['vids']:
            if v_['vid'] in vids and vids[v_['vid']]['unsorted']:
                INC[(v_['vid'], jx_)] = v_['conf']
    inc_for_vid = defaultdict(list)
    for (vid_, j2_) in INC:
        inc_for_vid[vid_].append(j2_)
    STAB = float(os.environ.get('F_STAB', '0.10'))
    for vid_, js_ in inc_for_vid.items():
        inc_set_ = set(js_)
        inc_best_ = max([best_sim.get((vid_, j2_), 0.0) for j2_ in js_] + [T_CLAIM - 0.02])
        for j_ in list(claims.get(vid_, {}).keys()):
            if j_ in inc_set_:
                continue
            if best_sim[(vid_, j_)] < inc_best_ + STAB:
                del claims[vid_][j_]
                best_sim.pop((vid_, j_), None)
                inc_dropped += 1
        if not claims.get(vid_):
            claims.pop(vid_, None)
    for (vid_, j2_), pconf_ in list(INC.items()):
        if claims.get(vid_, {}).get(j2_, 0) == 0:
            claims[vid_][j2_] = 1
            best_sim[(vid_, j2_)] = max(best_sim.get((vid_, j2_), 0.0), T_CLAIM - 0.001)
            inc_missing += 1
    print(f"incumbent guard (STAB={STAB}): dropped {inc_dropped} flips | re-added {inc_missing} incumbents")

# ---- 文件名血缘拉升（内容级，env F_LINEAGE=1 启用；不含目录/频道概念）----
#   FN_DUP: 同 basename_key + 同 size + |Δdur|<=1.5s 的另一副本已强认领同身份（s>=T_STRONG 或 n>=2）
#   FN_NAME: 文件名（去扩展后_key）包含身份名 key（长度>=3）
fn_ok = {}   # (vid, idj) -> reason
if os.environ.get('F_LINEAGE'):
    def _bkey(p_):
        stem_ = p_.rsplit('.', 1)[0] if '.' in p_ else p_
        return _name_key(stem_)

    by_base = defaultdict(list)
    for vid_, cnt_ in claims.items():
        by_base[_bkey(vids[vid_]['name'])].append(vid_)
    def _strong(vid_, j_):
        return claims[vid_].get(j_, 0) >= 2 or best_sim.get((vid_, j_), 0.0) >= T_STRONG
    for vid_, cnt_ in list(claims.items()):
        nm_base_ok = vids[vid_].get('unsorted', True)   # 仅未分类视频参与
        if not nm_base_ok:
            continue
        b_ = _bkey(vids[vid_]['name'])
        for j_ in cnt_:
            if _strong(vid_, j_):
                continue
            nm = registry2[j_]['name']
            if nm in ('未知', ''):
                continue
            reasons = []
            # FN_DUP
            for v2_ in by_base.get(b_, []):
                if v2_ == vid_ or not _strong(v2_, j_):
                    continue
                if vids[v2_]['size'] == vids[vid_]['size'] and \
                        abs(float(vids[v2_]['duration']) - float(vids[vid_]['duration'])) <= 1.5:
                    reasons.append('dup')
                    break
            # FN_NAME
            k_ = _name_key(nm)
            if len(k_) >= 3 and k_ in b_:
                reasons.append('name')
            if reasons:
                fn_ok[(vid_, j_)] = '+'.join(reasons)
    print(f"filename-lineage promotions: {len(fn_ok)}")

# ---- embedding 共识拉升（F_CONSENSUS=1：纯脸证据，无文件级信号）----
#   cs:mean   多帧共识：n>=3 且均值>=0.56 且该视频只断言这一个身份
#   cs:margin 差距重排：top1 均值-top2 均值>=0.08 且 top1>=0.52
#   cs:vote   全局聚类投票：本视频的脸与身份模板脸/强认领视频的脸落在同一全局聚类
if os.environ.get('F_CONSENSUS') and len(C):
    vid_faces = defaultdict(list)
    for i_ in unsorted_face:
        vid_faces[fvid[i_]].append(i_)

    def _pend(vid_, j_):
        return claims[vid_].get(j_, 0) == 1 and best_sim.get((vid_, j_), 0.0) < T_STRONG

    vid_sum = defaultdict(lambda: defaultdict(float))
    for fi, i_ in enumerate(unsorted_face):
        row = UF[fi]
        vid_ = fvid[i_]
        for idj in np.where(row >= T_CLAIM)[0]:
            vid_sum[vid_][int(idj)] += float(row[idj])
    n_mean = n_margin = 0
    for vid_, sdict in vid_sum.items():
        means = {j_: s_ / claims[vid_][j_] for j_, s_ in sdict.items()}
        order = sorted(means.items(), key=lambda kv: -kv[1])
        top_j, top_m = order[0]
        if _pend(vid_, top_j):
            if claims[vid_][top_j] >= 3 and top_m >= 0.56 and len(sdict) == 1:
                fn_ok[(vid_, top_j)] = 'cs:mean'; n_mean += 1; continue
            if len(order) >= 2 and top_m - order[1][1] >= 0.08 and top_m >= 0.52:
                fn_ok[(vid_, top_j)] = 'cs:margin'; n_margin += 1
    id_labels = defaultdict(set)
    for j_, r_ in enumerate(registry2):
        for i_ in r_['tpl']:
            id_labels[j_].add(int(labels[i_]))
    for v2_, cnt2_ in claims.items():
        for j2_ in cnt2_:
            if cnt2_[j2_] >= 2 or best_sim.get((v2_, j2_), 0.0) >= T_STRONG:
                for i_ in vid_faces.get(v2_, []):
                    id_labels[j2_].add(int(labels[i_]))
    n_vote = 0
    promoted = []
    for vid_, cnt_ in claims.items():
        labs_ = {int(labels[i_]) for i_ in vid_faces.get(vid_, [])}
        if not labs_:
            continue
        for j_ in list(cnt_):
            if not _pend(vid_, j_) or not (labs_ & id_labels[j_]):
                continue
            cand_amb = [j2_ for j2_ in cnt_ if j2_ != j_ and (labs_ & id_labels.get(j2_, set()))]
            strict_amb = [j2_ for j2_ in id_labels if j2_ != j_ and (labs_ & id_labels[j2_])]
            key = (vid_, j_)
            fn_ok[key] = (fn_ok.get(key, '') + '+cs:vote').lstrip('+')
            n_vote += 1
            promoted.append({'vid': vid_, 'id': int(j_), 'name': registry2[j_]['name'],
                             's': round(best_sim.get((vid_, j_), 0.0), 4),
                             'n': int(cnt_[j_]),
                             'cand_amb': len(cand_amb), 'strict_amb': len(strict_amb)})
    json.dump(promoted, open(f'{OUT}/promoted.json', 'w'), ensure_ascii=False)
    ex = [p for p in promoted if p['strict_amb'] == 0]
    multi = len({p['vid'] for p in promoted}) and sum(
        1 for v_, g_ in defaultdict(list, {**{}}).items() if False)  # placeholder
    vid_cnt = defaultdict(int)
    for p in promoted:
        vid_cnt[p['vid']] += 1
    dbl = sum(1 for v_, c_ in vid_cnt.items() if c_ >= 2)
    print(f"consensus promotions: mean={n_mean} margin={n_margin} vote={n_vote} (union {len(fn_ok)})")
    print(f"vote audit: exclusive={len(ex)} ambiguous={len(promoted)-len(ex)} | vids promoted to >=2 ids: {dbl}")


if os.environ.get('F_DUMP'):
    import os as _os
    dump = []
    for vid, cnt in claims.items():
        for idj, n in cnt.items():
            dump.append({'vid': vid, 'id': idj, 'name': registry2[idj]['name'],
                         'n': n, 's': best_sim[(vid, idj)],
                         'zone': 'MyTG' if 'My Telegram' in vid else 'share'})
    json.dump(dump, open(f'{OUT}/claims-detail.json', 'w'), ensure_ascii=False)
    print(f"dumped {len(dump)} claims -> {OUT}/claims-detail.json")

ident_out = []
claimed_vids = set()
for j, r in enumerate(registry2):
    mem = []
    for vid, cnt in claims.items():
        n = cnt.get(j, 0)
        if not n:
            continue
        s = best_sim[(vid, j)]
        fn = fn_ok.get((vid, j))
        conf = 'ok' if (n >= 2 or s >= T_STRONG or fn) else 'weak'
        if fn:
            conf = 'ok-fn'
        v = vids[vid]
        mem.append({'vid': vid, 'name': v['name'], 'size': v['size'],
                    'duration': v['duration'], 'thumb': v['thumb'], 'conf': conf})
        if fn:
            mem[-1]['fn'] = fn
    if not mem:
        continue
    mem.sort(key=lambda x: -x['size'])
    rep_face = max(r['tpl'], key=lambda i: fsc[i])
    gb = sum(m['size'] for m in mem)/2**30
    claimed_vids.update(m['vid'] for m in mem)
    ident_out.append({'id': f'id{j}', 'name': r['name'], 'aliases': r['aliases'],
                      'kind': r['kind'], 'tpl_faces': len(r['tpl']),
                      'folder_n': r['folder_n'], 'folder_gb': round(r['folder_gb'], 1),
                      'claimed_n': len(mem), 'claimed_gb': round(gb, 1),
                      'rep_thumb': fthumb[rep_face], 'vids': mem})
ident_out.sort(key=lambda x: -x['claimed_gb'])

# ---- 剩余未认领 → 无名聚类 ----
rest_face = [i for i in unsorted_face if fvid[i] not in claimed_vids]
rest_vids = {fvid[i] for i in rest_face}
print(f"unclaimed unsorted: {len(rest_vids)}v ({sum(vids[v]['size'] for v in rest_vids)/2**30:.0f}G)")
labels_rest = cluster_embeddings(E[rest_face]) if rest_face else np.zeros(0, np.int32)
rest_groups = defaultdict(list)
for k_, i_local in enumerate(rest_face):
    rest_groups[labels_rest[k_]].append(i_local)

unknown = []
for r_, idxs in rest_groups.items():
    gv = {fvid[i] for i in idxs}
    if len(idxs) < 3 or len(gv) < 2:
        continue
    mem = []
    for vid in gv:
        v = vids[vid]
        mem.append({'vid': vid, 'name': v['name'], 'size': v['size'],
                    'duration': v['duration'], 'thumb': v['thumb'], 'conf': 'ok'})
    mem.sort(key=lambda x: -x['size'])
    rep_i = max(idxs, key=lambda i: fsc[i])
    unknown.append({'cid': f'u{r_}', 'n_videos': len(mem),
                    'total_gb': round(sum(m['size'] for m in mem)/2**30, 1),
                    'n_faces': len(idxs), 'rep_thumb': fthumb[rep_i],
                    'collections': [], 'vids': mem})
unknown.sort(key=lambda c: -c['total_gb'])

report = {
    'stats': {
        'scanned': len(vids),
        'unsorted_n': sum(1 for v in vids.values() if v['unsorted']),
        'unsorted_gb': round(sum(v['size'] for v in vids.values() if v['unsorted'])/2**30, 1),
        'identities': len(ident_out),
        'claimed_n': len(claimed_vids),
        'claimed_gb': round(sum(vids[v]['size'] for v in claimed_vids)/2**30, 1),
        'unknown_clusters': len(unknown),
        'unknown_n': sum(c['n_videos'] for c in unknown),
        'unknown_gb': round(sum(c['total_gb'] for c in unknown), 1),
    },
    'identities': ident_out,
    'unknown_clusters': unknown,
}
json.dump(report, open(f'{OUT}/report.json', 'w'), ensure_ascii=False)
print(f"\n=== 报告 ===")
s = report['stats']
print(f"未分类 {s['unsorted_n']}v/{s['unsorted_gb']}G → 认领 {s['claimed_n']}v/{s['claimed_gb']}G 到 {s['identities']} 人 | 无名 {s['unknown_clusters']} 组 {s['unknown_n']}v/{s['unknown_gb']}G")
print("\nTop10 认领人物:")
for it in ident_out[:10]:
    al = f" (={','.join(it['aliases'])})" if it['aliases'] else ''
    ex = it['vids'][0]['name'][:26] if it['vids'] else ''
    print(f"  {it['claimed_gb']:7.1f}G {it['claimed_n']:4d}v  {it['name'][:20]}{al}  e.g. {ex}")
print(f"\ndone in {time.time()-t0:.0f}s")
