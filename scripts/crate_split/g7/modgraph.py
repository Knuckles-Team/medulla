"""Module graph of one crate (G7 split, MEDULLA-CONTRACT-R056.11). usage (repo root): modgraph.py <crate> [expand_mod ...] > graph.json"""
import sys,re,json,importlib.util
from pathlib import Path
from collections import defaultdict
sys.path.insert(0,'scripts')
spec=importlib.util.spec_from_file_location('m','scripts/crate_split/move_leaf_modules.py'); m=importlib.util.module_from_spec(spec); spec.loader.exec_module(m)
crate=sys.argv[1]; expand=set(sys.argv[2:])
src=Path(f'crates/{crate}/src')
top=m.list_mods(src); rn=m.root_names((src/'lib.rs').read_text())
units={}  # unit name -> files
for t,ps in top.items():
    if t in expand:
        d=src/t
        kids=m.list_mods(d)
        for k,kp in kids.items(): units[f'{t}::{k}']=m.files_of(kp)
        own=[p for p in ps if p.is_file()]+[d/'mod.rs'] if (d/'mod.rs').exists() else [p for p in ps if p.is_file()]
        units[t]=[p for p in own if p.exists()]
    else: units[t]=m.files_of(ps)
REEX={}
for t in expand:
    pf=[f for f in units.get(t,[])]
    for f in pf:
        code=m.blank_non_code(f.read_text())
        for _s,_e,_i,body in m.find_use_statements(code):
            for segs,_l,_a in m.parse_use_tree(body):
                segs=list(segs)
                if segs and segs[0] in ('self','crate'): segs=segs[1:] if segs[0]=='self' else segs[2:] if len(segs)>1 and segs[1]==t else []
                if len(segs)>=2 and f'{t}::{segs[0]}' in units: REEX[(t,segs[-1])]=f'{t}::{segs[0]}'
        # glob re-exports: pub use child::*
        for x in re.finditer(r'pub(?:\(crate\))? use (?:self::)?(\w+)::\*;',code):
            if f'{t}::{x.group(1)}' in units: REEX[(t,'*'+x.group(1))]=f'{t}::{x.group(1)}'
lines={u:sum(len(f.read_text().splitlines()) for f in fs) for u,fs in units.items()}
def unit_of(segs):
    if not segs: return None,None
    h=segs[0]
    if h not in top and h in rn: h=rn[h]
    if h not in top: return None,None
    if h in expand and len(segs)>1 and f'{h}::{segs[1]}' in units: return f'{h}::{segs[1]}','::'.join(segs[2:]) or '*'
    if h in expand and len(segs)>1 and (h,segs[1]) in REEX: return REEX[(h,segs[1])],'::'.join(segs[1:])
    return h,'::'.join(segs[1:]) or '*'
SUP=re.compile(r'(?<![\w:])super((?:::[A-Za-z_]\w*)+)')
E=defaultdict(lambda: defaultdict(set))
for u,fs in units.items():
    for f in fs:
        code=m.blank_non_code(f.read_text())
        segsets=[x.group(1).split('::')[1:] for x in m.CRATE_PATH.finditer(code)]
        segsets+=[x.group(1).split('::')[1:] for x in m.DCRATE_PATH.finditer(code)]
        for _s,_e,_i,body in m.find_use_statements(code):
            for segs,_l,_a in m.parse_use_tree(body):
                if segs and segs[0]=='crate': segsets.append(list(segs[1:]))
        # super:: from a direct child of an expanded module
        if '::' in u and f.parent==src/u.split('::')[0] or (('::' in u) and f.name=='mod.rs' and f.parent.parent==src/u.split('::')[0]):
            par=u.split('::')[0]
            for x in SUP.finditer(code): segsets.append([par]+x.group(1).split('::')[1:])
        if u in expand:
            for x in re.finditer(r'(?<![\w:])(\w+)((?:::\w+)+)',code):
                if f'{u}::{x.group(1)}' in units: segsets.append([u,x.group(1)]+x.group(2).split('::')[1:])
        for segs in segsets:
            d,item=unit_of(segs)
            if d and d!=u: E[u][d].add(item)
json.dump({'lines':lines,'edges':{a:{b:sorted(v) for b,v in d.items()} for a,d in E.items()}},sys.stdout,indent=0)
