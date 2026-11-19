# Draws the Conflux icon: three streams flowing into one play button.
import zlib, struct, sys, math

BG=(13,15,20); C=[(255,184,77),(94,230,201),(167,139,250)]; W=(244,246,250)

def seg_dist(px,py,ax,ay,bx,by):
    dx,dy=bx-ax,by-ay; t=max(0,min(1,((px-ax)*dx+(py-ay)*dy)/(dx*dx+dy*dy)))
    return math.hypot(px-(ax+t*dx),py-(ay+t*dy))

def curve(i):
    # quadratic bezier from left edge into the center, sampled as segments
    y0=[0.30,0.50,0.70][i]; pts=[]
    for k in range(25):
        t=k/24; x=(1-t)**2*0.10+2*(1-t)*t*0.38+t*t*0.56
        y=(1-t)**2*y0+2*(1-t)*t*y0+t*t*0.50
        pts.append((x,y))
    return pts
CURVES=[curve(i) for i in range(3)]

def in_tri(px,py):
    # play triangle centered at (0.66,0.5)
    a=(0.56,0.36);b=(0.56,0.64);c=(0.80,0.50)
    def s(p1,p2,p3): return (p1[0]-p3[0])*(p2[1]-p3[1])-(p2[0]-p3[0])*(p1[1]-p3[1])
    d1=s((px,py),a,b);d2=s((px,py),b,c);d3=s((px,py),c,a)
    neg=(d1<0)or(d2<0)or(d3<0);pos=(d1>0)or(d2>0)or(d3>0)
    return not(neg and pos)

def color(x,y,rounded,scale=1.0):
    x=0.5+(x-0.5)/scale; y=0.5+(y-0.5)/scale
    if rounded:
        r=0.22; cx=min(max(x,r),1-r); cy=min(max(y,r),1-r)
        if math.hypot(x-cx,y-cy)>r: return None
    if in_tri(x,y): return W
    for i,pts in enumerate(CURVES):
        for (ax,ay),(bx,by) in zip(pts,pts[1:]):
            if seg_dist(x,y,ax,ay,bx,by)<0.045: return C[i]
    return BG

def png(size,path,rounded,ss=3,scale=1.0):
    rows=[]
    for j in range(size):
        row=bytearray([0])
        for i in range(size):
            acc=[0,0,0,0]
            for sj in range(ss):
                for si in range(ss):
                    c=color((i+(si+.5)/ss)/size,(j+(sj+.5)/ss)/size,rounded,scale)
                    if c: acc[0]+=c[0];acc[1]+=c[1];acc[2]+=c[2];acc[3]+=255
            n=ss*ss; a=acc[3]//n
            if a: row+=bytes([acc[0]*255//acc[3],acc[1]*255//acc[3],acc[2]*255//acc[3],a])
            else: row+=bytes([0,0,0,0])
        rows.append(bytes(row))
    raw=b''.join(rows)
    def chunk(t,d): return struct.pack('>I',len(d))+t+d+struct.pack('>I',zlib.crc32(t+d)&0xffffffff)
    data=b'\x89PNG\r\n\x1a\n'+chunk(b'IHDR',struct.pack('>IIBBBBB',size,size,8,6,0,0,0))+chunk(b'IDAT',zlib.compress(raw,9))+chunk(b'IEND',b'')
    open(path,'wb').write(data)

out=sys.argv[1]
png(192,out+'/icon-192.png',True)
png(512,out+'/icon-512.png',True,2)
png(512,out+'/icon-maskable-512.png',False,2,0.78)
png(180,out+'/apple-touch-icon.png',False,3,0.88)  # iOS rounds corners itself
png(32,out+'/favicon-32.png',True,4)
