"""共享的人脸分组器：互近邻(k)连通分量。server 与 identify_faces 共用，保证组序号一致。"""
import numpy as np
from collections import defaultdict


def mutual_groups(E, T=0.47, k=4):
    """互近邻连通分量。返回 list[list[int]]（局部索引，按组大小降序）。"""
    n = len(E)
    if n == 0:
        return []
    S = E @ E.T
    k = min(k, n - 1)
    if k <= 0:
        return [[i] for i in range(n)]
    idx = np.argpartition(-S, k, axis=1)[:, :k]
    par = list(range(n))

    def find(x):
        while par[x] != x:
            par[x] = par[par[x]]
            x = par[x]
        return x

    for i in range(n):
        for j in idx[i]:
            if S[i, j] >= T and i in idx[j]:
                ra, rb = find(i), find(j)
                if ra != rb:
                    par[ra] = rb
    g = defaultdict(list)
    for i in range(n):
        g[find(i)].append(i)
    return sorted(g.values(), key=len, reverse=True)
