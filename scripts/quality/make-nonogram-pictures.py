#!/usr/bin/env python3
"""Reproduce the original Nonograms picture pack and independently verify line solvability.

The five-square masters below were drawn for Cobalt. Larger grids use explicit
nearest-cell scaling; no external puzzle data, art, library or generator is used.
"""
from functools import lru_cache
from pathlib import Path
import hashlib

ART = {'House': ['..#..', '.###.', '#####', '##.##', '##.##'],
 'Arrow': ['..#..', '.###.', '#####', '..#..', '..#..'],
 'Heart': ['.#.#.', '#####', '#####', '.###.', '..#..'],
 'Tree': ['..#..', '.###.', '#####', '..#..', '.###.'],
 'Cup': ['####.', '#..##', '#..##', '####.', '.....'],
 'Key': ['.###.', '.#.#.', '.###.', '..#..', '..##.'],
 'Fish': ['..##.', '.####', '####.', '.####', '..##.'],
 'Flag': ['####.', '#####', '####.', '#....', '#....'],
 'Moon': ['.###.', '##...', '##...', '##...', '.###.'],
 'Diamond': ['..#..', '.###.', '#####', '.###.', '..#..'],
 'Tile': ['.###.', '#####', '#####', '#####', '.###.'],
 'Rocket': ['..#..', '.###.', '.#.#.', '#####', '#.#.#'],
 'Cat': ['#...#', '##.##', '#####', '#.#.#', '.###.'],
 'Umbrella': ['..#..', '.###.', '#####', '..#..', '.##..'],
 'Mushroom': ['.###.', '#####', '#####', '..#..', '.###.'],
 'Anchor': ['..#..', '.###.', '..#..', '#.#.#', '.###.'],
 'Castle': ['#.#.#', '#####', '#####', '##.##', '##.##'],
 'Bridge': ['#####', '#####', '#.#.#', '#.#.#', '#.#.#']}

def runs(line):
 out=[];n=0
 for v in list(line)+[False]:
  if v:n+=1
  elif n:out.append(n);n=0
 return tuple(out)
@lru_cache(None)
def candidates(n,rs):
 if not rs:return (0,)
 k=rs[0]; tail=rs[1:]; need=sum(tail)+len(tail)
 out=[]
 for start in range(n-k-need+1):
  mask=((1<<k)-1)<<start
  if not tail:out.append(mask)
  else:
   for rest in candidates(n-start-k-1,tail):out.append(mask|(rest<<(start+k+1)))
 return tuple(out)
def solve(a):
 n=len(a); row=[candidates(n,runs(r)) for r in a];col=[candidates(n,runs(a[r][c] for r in range(n))) for c in range(n)]
 board=[[None]*n for _ in range(n)];rounds=0
 while True:
  changed=False
  for vertical,groups in [(False,row),(True,col)]:
   for i,cs in enumerate(groups):
    line=[board[j][i] if vertical else board[i][j] for j in range(n)]
    yes=sum(1<<j for j,v in enumerate(line) if v is True);no=sum(1<<j for j,v in enumerate(line) if v is False)
    cs=[v for v in cs if v&yes==yes and not v&no];groups[i]=cs
    if not cs:return False,rounds
    all_yes=(1<<n)-1; any_yes=0
    for v in cs:all_yes&=v;any_yes|=v
    for j in range(n):
     forced=True if all_yes&(1<<j) else False if not(any_yes&(1<<j)) else None
     r,c=(j,i) if vertical else(i,j)
     if board[r][c] is None and forced is not None: board[r][c]=forced;changed=True
  if not changed:return board==a,rounds
  rounds+=1

def main():
    entries=[]
    for index,(title,art) in enumerate(ART.items()):
        side=[5,7,9,15,25][min(index//4,4)]
        rows=[''.join(art[y*5//side][x*5//side] for x in range(side)) for y in range(side)]
        assert solve([[cell=='#' for cell in row] for row in rows])[0], title
        entries.append('|'.join([title.lower(),title,str(side),''.join(rows)]))
    output=Path(__file__).resolve().parents[2]/'apps/nonograms/assets/pictures.txt'
    output.write_text('\n'.join(entries)+'\n')
    print(f'{len(entries)} original pictures: {hashlib.sha256(output.read_bytes()).hexdigest()}')

if __name__=='__main__':
    main()
