#!/usr/bin/env python3
"""Generate original Logic Pack puzzles. No external puzzle corpus or code.

Loop, bridge and cross-sum puzzles must have exactly one rule-valid solution.
Mines fields use published fixed seeds and density-based difficulty; they do
not promise guess-free play. The original four identities remain first.
"""
import argparse
import itertools
import json
import random
from pathlib import Path

ROOT=Path(__file__).resolve().parents[2]

def loop_edges(side):
    width=side+1
    return [(r*width+c,r*width+c+1) for r in range(width) for c in range(side)]+[(r*width+c,(r+1)*width+c) for r in range(side) for c in range(width)]

def connected(edges,values,vertices):
    present={i for i in range(vertices) if any(v and i in e for e,v in zip(edges,values))}
    if not present:return False
    seen={next(iter(present))}
    while True:
        newer=seen|{b for (a,b),v in zip(edges,values) if v and a in seen}|{a for (a,b),v in zip(edges,values) if v and b in seen}
        if newer==seen:return seen==present
        seen=newer

def bounded_solve(domains,constraints,accept,limit=2):
    """Constraint propagation plus complete bounded search; no solution template."""
    found=[]
    def visit(current):
        while True:
            changed=False
            for positions,allowed in constraints:
                lows=[min(current[i]) for i in positions]; highs=[max(current[i]) for i in positions]
                targets=[t for t in allowed if sum(lows)<=t<=sum(highs)]
                if not targets:return
                for offset,i in enumerate(positions):
                    values={v for v in current[i] if any(sum(lows)-lows[offset]+v<=t<=sum(highs)-highs[offset]+v for t in targets)}
                    if not values:return
                    if values!=current[i]:current[i]=values;changed=True
            if not changed:break
        unresolved=[i for i,vs in enumerate(current) if len(vs)>1]
        if not unresolved:
            answer=[next(iter(vs)) for vs in current]
            if accept(answer):found.append(answer)
            return
        i=min(unresolved,key=lambda i:len(current[i]))
        for value in sorted(current[i]):
            branch=[vs.copy() for vs in current];branch[i]={value};visit(branch)
            if len(found)>=limit:return
    visit([set(vs) for vs in domains])
    return found

def loop_solutions(side,clues):
    edges=loop_edges(side); vertices=(side+1)**2
    rules=[([i for i,e in enumerate(edges) if v in e],(0,2)) for v in range(vertices)]
    for r in range(side):
        for c in range(side):
            n=clues[r*side+c]
            if n is not None:rules.append(([r*side+c,(r+1)*side+c,side*(side+1)+r*(side+1)+c,side*(side+1)+r*(side+1)+c+1],(n,)))
    return bounded_solve([{0,1} for _ in edges],rules,lambda answer:connected(edges,answer,vertices))

def loop_from_region(side,region):
    answer=[]
    for r in range(side+1):
        for c in range(side):answer.append(int(((r-1,c) in region)!=((r,c) in region)))
    for r in range(side):
        for c in range(side+1):answer.append(int(((r,c-1) in region)!=((r,c) in region)))
    clues=[sum(answer[i] for i in [r*side+c,(r+1)*side+c,side*(side+1)+r*(side+1)+c,side*(side+1)+r*(side+1)+c+1]) for r in range(side) for c in range(side)]
    return answer,clues

def bridge_routes(islands):
    routes=[]
    for i,(x,y,_) in enumerate(islands):
        for direction in ('right','down'):
            candidates=[j for j,(a,b,_) in enumerate(islands) if (b==y and a>x if direction=='right' else a==x and b>y)]
            if candidates:routes.append((i,min(candidates,key=lambda j:abs(islands[j][0]-x)+abs(islands[j][1]-y))))
    return routes

def bridge_solutions(islands,routes):
    rules=[([i for i,e in enumerate(routes) if v in e],(n,)) for v,(_,_,n) in enumerate(islands)]
    def valid(answer):
        if not connected(routes,answer,len(islands)):return False
        for i,(a,b) in enumerate(routes):
            if not answer[i]:continue
            ax,ay,_=islands[a];bx,by,_=islands[b]
            for j,(c,d) in enumerate(routes[:i]):
                if not answer[j]:continue
                cx,cy,_=islands[c];dx,dy,_=islands[d]
                if ax==bx and cy==dy and min(cx,dx)<ax<max(cx,dx) and min(ay,by)<cy<max(ay,by):return False
                if ay==by and cx==dx and min(ax,bx)<cx<max(ax,bx) and min(cy,dy)<ay<max(cy,dy):return False
        return True
    return bounded_solve([{0,1,2} for _ in routes],rules,valid)

def cross_runs(mask):
    height=len(mask);width=len(mask[0]);white=[(r,c) for r in range(height) for c in range(width) if mask[r][c]=='.']; lookup={p:i for i,p in enumerate(white)}
    runs=[]
    for r,c in white:
        for dr,dc in [(0,1),(1,0)]:
            if (r-dr,c-dc) in lookup:continue
            cells=[];rr,cc=r,c
            while (rr,cc) in lookup:cells.append(lookup[(rr,cc)]);rr+=dr;cc+=dc
            if len(cells)<2:raise ValueError('single-square run')
            runs.append({'cells':cells,'clue':[c-dc,r-dr],'down':bool(dr)})
    return white,runs

def cross_solutions(mask,runs,givens,limit=2):
    white,_=cross_runs(mask); candidates=[]
    for run in runs:
        candidates.append([values for values in itertools.permutations(range(1,10),len(run['cells'])) if sum(values)==run['sum'] and all(not givens[i] or givens[i]==v for i,v in zip(run['cells'],values))])
    found=[]
    def visit(active):
        while True:
            options=[set(range(1,10)) for _ in white]
            for run,choices in zip(runs,active):
                if not choices:return
                for offset,i in enumerate(run['cells']):options[i]&={v[offset] for v in choices}
            if any(not opts for opts in options):return
            reduced=[[v for v in choices if all(n in options[i] for i,n in zip(run['cells'],v))] for run,choices in zip(runs,active)]
            if reduced==active:break
            active=reduced
        uncertain=[i for i,opts in enumerate(options) if len(opts)>1]
        if not uncertain:found.append([next(iter(opts)) for opts in options]);return
        i=min(uncertain,key=lambda i:len(options[i]))
        for n in sorted(options[i]):
            visit([[v for v in choices if i not in run['cells'] or v[run['cells'].index(i)]==n] for run,choices in zip(runs,active)])
            if len(found)>=limit:return
    visit(candidates)
    return found

def puzzle(id,kind,title,difficulty,guide,**rules):
    return dict(id=id,kind=kind,title=title,difficulty=difficulty,guide=guide,**rules)

def build():
    rng=random.Random(20260909)
    answer=[int(bool(0b101101110011&(1<<i))) for i in range(12)]
    original_islands=[[2,0,1],[0,2,1],[2,2,4],[4,2,1],[2,4,1]]
    # Original route ordering is part of stored progress.
    original_routes=[[0,2],[1,2],[3,2],[4,2]]
    _,runs=cross_runs(['###','#..','#..'])
    original_answer=[1,3,2,4]
    for run in runs:run['sum']=sum(original_answer[i] for i in run['cells'])
    pack=[puzzle('slither-four-twos','slither','Four twos','Starter','A 2 × 2 board with every clue shown.',side=2,clues=[2]*4,solution=answer),
        puzzle('hashi-cross','hashi','Five islands','Starter','One central island connects four neighbours.',width=5,height=5,islands=original_islands,routes=original_routes,solution=[1]*4),
        puzzle('kakuro-three-cells','kakuro','Three blanks','Starter','Three blanks and one printed digit.',mask=['###','#..','#..'],runs=runs,givens=[1,0,0,0],solution=original_answer),
        puzzle('mines-four-square','mines','Small field','Starter','A 4 × 4 field with three mines; the first reveal is safe.',side=4,mines=[5,10,15])]
    loop_regions=[(3,{(0,0),(0,1),(1,0),(1,1),(2,1)}),(3,{(0,1),(0,2),(1,0),(1,1),(1,2),(2,0)}),
                  (4,{(0,0),(0,1),(1,0),(1,1),(1,2),(2,1),(2,2),(3,1)}),
                  (4,{(0,1),(0,2),(0,3),(1,0),(1,1),(1,2),(1,3),(2,0),(2,1),(3,0),(3,1),(3,2)})]
    for i,(side,region) in enumerate(loop_regions):
        answer,clues=loop_from_region(side,region)
        assert loop_solutions(side,clues)==[answer]
        if i>=2:
            order=list(range(side*side));rng.shuffle(order)
            for position in order:
                previous=clues[position];clues[position]=None
                if loop_solutions(side,clues)!=[answer]:clues[position]=previous
                if clues.count(None)>=4:break
        pack.append(puzzle(f'slither-{i+1:02}','slither',['First bend','Around the corner','Narrow passage','Long way round'][i], 'Easy' if i<2 else 'Medium',
            f'{side} × {side} cells; '+('all clues shown.' if i<2 else 'some clues omitted; use loop continuity.'),side=side,clues=clues,solution=answer))
    for i,(cols,rows) in enumerate([(3,2),(3,3),(4,3),(4,4)]):
        islands=[[x*2,y*2,0] for y in range(rows) for x in range(cols)];routes=bridge_routes(islands)
        for attempt in range(20000):
            answer=[rng.choices([0,1,2],[2,3,5])[0] for _ in routes]
            if not connected(routes,answer,len(islands)):continue
            for vertex,island in enumerate(islands):island[2]=sum(n for e,n in zip(routes,answer) if vertex in e)
            if any(not island[2] for island in islands):continue
            if bridge_solutions(islands,routes)==[answer]:break
        else:raise RuntimeError('Cannot find unique bridge puzzle')
        pack.append(puzzle(f'hashi-{i+1:02}','hashi',['Six islands','Nine islands','Across the bay','Island chain'][i],'Easy' if i<2 else 'Medium',
            f'{len(islands)} islands. Double bridges and a connected network are required.',width=cols*2-1,height=rows*2-1,islands=[row.copy() for row in islands],routes=routes,solution=answer))
    masks=[['####','#...','#...','#...'],['#####','##...','#....','#....','#...#'],
           ['#####','#....','#....','#....','#....'],['######','##....','#.....','#.....','#....#','######']]
    for i,mask in enumerate(masks):
        white,runs=cross_runs(mask)
        digits=list(range(1,10));rng.shuffle(digits)
        answer=[digits[(r+c*2)%9] for r,c in white]
        for run in runs:run['sum']=sum(answer[cell] for cell in run['cells'])
        givens=answer.copy();order=list(range(len(white)));rng.shuffle(order)
        for cell in order:
            givens[cell]=0
            if cross_solutions(mask,runs,givens)!=[answer]:givens[cell]=answer[cell]
        pack.append(puzzle(f'kakuro-{i+1:02}','kakuro',['Small sums','Corner clues','Four by four','Long runs'][i],'Easy' if i<2 else 'Medium',
            f'{len(white)} white cells, {sum(bool(v) for v in givens)} printed digits; run lengths up to {max(len(r["cells"]) for r in runs)}.',mask=mask,runs=runs,givens=givens,solution=answer))
    for i,(side,count) in enumerate([(5,4),(5,6),(6,8),(7,11)]):
        mines=sorted(rng.sample(range(side*side),count))
        pack.append(puzzle(f'mines-{i+1:02}','mines',['Room to explore','Closer neighbours','Wide field','Far corners'][i],'Easy' if i<2 else 'Medium',
            f'{side} × {side} cells with {count} mines. The first reveal is safe; some positions may require a guess.',side=side,mines=mines))
    return pack

def verify(pack):
    assert len(pack)==20 and len({p['id'] for p in pack})==20
    for p in pack:
        if p['kind']=='slither':assert loop_solutions(p['side'],p['clues'])==[p['solution']],p['id']
        elif p['kind']=='hashi':assert bridge_solutions(p['islands'],p['routes'])==[p['solution']],p['id']
        elif p['kind']=='kakuro':assert cross_solutions(p['mask'],p['runs'],p['givens'])==[p['solution']],p['id']
        else:assert len(p['mines'])==len(set(p['mines'])) and all(0<=m<p['side']**2 for m in p['mines'])

if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--check',action='store_true');args=parser.parse_args()
    path=ROOT/'apps/logicpack/assets/collection.json'
    if args.check:
        pack=json.loads(path.read_text());verify(pack);assert json.loads(json.dumps(build()))==pack,'Generated collection differs'
        print('Verified 20 original puzzles and deterministic reproduction.')
    else:
        pack=build();verify(pack);path.parent.mkdir(parents=True,exist_ok=True);path.write_text(json.dumps(pack,indent=2,ensure_ascii=False)+'\n');print('Wrote 20 original puzzles.')
