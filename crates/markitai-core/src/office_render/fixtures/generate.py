"""Original small Office fixtures; uses only Python's standard library to author ZIP/XML."""
from pathlib import Path
from zipfile import ZipFile, ZipInfo, ZIP_DEFLATED
import hashlib, json

ROOT=Path(__file__).resolve().parent
P='http://schemas.openxmlformats.org/presentationml/2006/main'
A='http://schemas.openxmlformats.org/drawingml/2006/main'
R='http://schemas.openxmlformats.org/officeDocument/2006/relationships'
PKG='http://schemas.openxmlformats.org/package/2006/relationships'
W='http://schemas.openxmlformats.org/wordprocessingml/2006/main'

def package(name, parts):
    with ZipFile(ROOT/name,'w') as z:
        for path,text in parts.items():
            info=ZipInfo(path,(2026,1,1,0,0,0)); info.compress_type=ZIP_DEFLATED
            z.writestr(info,text.encode())

def rels(entries):
    return f'<Relationships xmlns="{PKG}">'+''.join(f'<Relationship Id="{i}" Type="{R}/{typ}" Target="{target}"/>' for i,typ,target in entries)+'</Relationships>'

def box(n,x,y,w,h,color,text=''):
    body=f'<p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US" sz="3200"/><a:t>{text}</a:t></a:r></a:p></p:txBody>' if text else ''
    return f'<p:sp><p:nvSpPr><p:cNvPr id="{n}" name="Box {n}"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="{x}" y="{y}"/><a:ext cx="{w}" cy="{h}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="{color}"/></a:solidFill><a:ln><a:noFill/></a:ln></p:spPr>{body}</p:sp>'

group='<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/><a:chOff x="0" y="0"/><a:chExt cx="0" cy="0"/></a:xfrm></p:grpSpPr>'
parts={'_rels/.rels':rels([('rId1','officeDocument','ppt/presentation.xml')]),
'ppt/presentation.xml':f'<p:presentation xmlns:p="{P}" xmlns:a="{A}" xmlns:r="{R}"><p:sldIdLst>'+''.join(f'<p:sldId id="{256+i}" r:id="rId{i}"/>' for i in range(1,4))+'</p:sldIdLst><p:sldSz cx="9144000" cy="6858000" type="screen4x3"/><p:notesSz cx="6858000" cy="9144000"/></p:presentation>',
'ppt/_rels/presentation.xml.rels':rels([(f'rId{i}','slide',f'slides/slide{i}.xml') for i in range(1,4)])}
for i in range(1,4):
    # Slide two is hidden, slide three is intentionally blank.
    shapes=''
    if i<3:
        colors=['FF0000','00FF00','0000FF','FFFF00'] if i==1 else ['00FFFF','FF00FF','00FF00','FF0000']
        for j,(x,y) in enumerate([(0,0),(8229600,0),(0,5943600),(8229600,5943600)]):
            shapes+=box(j+2,x,y,914400,914400,colors[j])
        shapes+=box(6,1371600,2743200,6400800,1371600,'FFFFFF','VISIBLE FIRST' if i==1 else 'HIDDEN SECOND')
    parts[f'ppt/slides/slide{i}.xml']=f'<p:sld xmlns:p="{P}" xmlns:a="{A}" xmlns:r="{R}" show="{0 if i==2 else 1}"><p:cSld><p:spTree>{group}{shapes}</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sld>'
parts['[Content_Types].xml']='<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/>'+''.join(f'<Override PartName="/ppt/slides/slide{i}.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/>' for i in range(1,4))+'</Types>'
package('hidden-blank-three.pptx',parts)
# Separate paragraphs retain an explicit empty middle page in Writer import;
# consecutive breaks inside one paragraph are folded by the tested importer.
parts={'_rels/.rels':rels([('rId1','officeDocument','word/document.xml')]),
'[Content_Types].xml':'<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>',
'word/document.xml':f'<w:document xmlns:w="{W}"><w:body><w:p><w:r><w:rPr><w:sz w:val="40"/></w:rPr><w:t>WORD FIRST PAGE</w:t><w:br w:type="page"/></w:r></w:p><w:p><w:r><w:br w:type="page"/></w:r></w:p><w:p><w:r><w:rPr><w:sz w:val="40"/></w:rPr><w:t>WORD THIRD PAGE</w:t></w:r></w:p><w:sectPr><w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440"/></w:sectPr></w:body></w:document>'}
package('blank-middle-three.docx',parts)
record={'provenance':'Original fixtures authored for Markitai Rust; project license applies. Generated with Python standard library ZIP/XML only.', 'files':{p.name:{'bytes':p.stat().st_size,'sha256':hashlib.sha256(p.read_bytes()).hexdigest()} for p in ROOT.glob('*') if p.suffix in ('.pptx','.docx')},'expected':{'hidden-blank-three.pptx':{'pages':3,'size_points':[720,540],'slide_1':'VISIBLE FIRST plus red/green/blue/yellow corners','slide_2':'hidden source slide, HIDDEN SECOND plus cyan/magenta/green/red corners','slide_3':'blank white'},'blank-middle-three.docx':{'pages':3,'size_points':[612,792],'page_1':'WORD FIRST PAGE','page_2':'blank','page_3':'WORD THIRD PAGE'}}}
(ROOT/'provenance.json').write_text(json.dumps(record,indent=2)+'\n')
