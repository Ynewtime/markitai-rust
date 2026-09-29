"""Original deterministic workbook fixtures, authored with standard-library ZIP/XML.

The first sheet has a print area and manual breaks that deliberately exclude its
far bottom-right cell. Whole-sheet export must retain that cell and ignore the
paper pagination. The other sheets are hidden, truly empty, and visible, in order.
"""
from pathlib import Path
from zipfile import ZipFile, ZipInfo, ZIP_DEFLATED, ZIP_STORED
import hashlib
import json

ROOT = Path(__file__).resolve().parent
S = "http://schemas.openxmlformats.org/spreadsheetml/2006/main"
R = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
P = "http://schemas.openxmlformats.org/package/2006/relationships"
NAMES = ["Wide 宽表", "Hidden", "Empty", "Last"]


def package(path, parts):
    with ZipFile(path, "w") as archive:
        for name, value in parts.items():
            info = ZipInfo(name, (2026, 1, 1, 0, 0, 0))
            info.compress_type = ZIP_STORED if name == "mimetype" else ZIP_DEFLATED
            archive.writestr(info, value.encode())


def cell(ref, value, style):
    return f'<c r="{ref}" s="{style}" t="inlineStr"><is><t>{value}</t></is></c>'


parts = {
    "_rels/.rels": f'<Relationships xmlns="{P}"><Relationship Id="rId1" Type="{R}/officeDocument" Target="xl/workbook.xml"/></Relationships>',
    "[Content_Types].xml": '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/>' + ''.join(f'<Override PartName="/xl/worksheets/sheet{i}.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>' for i in range(1, 5)) + '</Types>',
    "xl/workbook.xml": f'<workbook xmlns="{S}" xmlns:r="{R}"><bookViews><workbookView activeTab="3"/></bookViews><sheets>' + ''.join(f'<sheet name="{name}" sheetId="{i}" state="{"hidden" if i == 2 else "visible"}" r:id="rId{i}"/>' for i, name in enumerate(NAMES, 1)) + '</sheets><definedNames><definedName name="_xlnm.Print_Area" localSheetId="0">\'Wide 宽表\'!$A$1:$D$10</definedName></definedNames></workbook>',
    "xl/_rels/workbook.xml.rels": f'<Relationships xmlns="{P}">' + ''.join(f'<Relationship Id="rId{i}" Type="{R}/worksheet" Target="worksheets/sheet{i}.xml"/>' for i in range(1, 5)) + f'<Relationship Id="style" Type="{R}/styles" Target="styles.xml"/></Relationships>',
    "xl/styles.xml": f'<styleSheet xmlns="{S}"><fonts count="1"><font><sz val="18"/><name val="Liberation Sans"/></font></fonts><fills count="6"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill>' + ''.join(f'<fill><patternFill patternType="solid"><fgColor rgb="FF{color}"/><bgColor indexed="64"/></patternFill></fill>' for color in ['FF0000', '0000FF', '00FF00', 'FFCC00']) + '</fills><borders count="1"><border/></borders><cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs><cellXfs count="5"><xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/>' + ''.join(f'<xf numFmtId="0" fontId="0" fillId="{fill}" borderId="0" xfId="0" applyFill="1"/>' for fill in range(2, 6)) + '</cellXfs><cellStyles count="1"><cellStyle name="Normal" xfId="0" builtinId="0"/></cellStyles></styleSheet>',
}
for index in range(1, 5):
    rows = ''
    if index == 1:
        for row in range(1, 35):
            values = ''
            if row == 1:
                values = cell('A1', 'VISIBLE FIRST', 1)
            if row == 31:
                values = cell('J31', 'OUTSIDE PRINT AREA', 2)
            rows += f'<row r="{row}" ht="24" customHeight="1">{values}</row>'
    elif index != 3:
        rows = '<row r="1" ht="40" customHeight="1">' + cell('A1', 'HIDDEN MIDDLE' if index == 2 else 'LAST SHEET', 3 if index == 2 else 4) + '</row>'
    columns = '<cols><col min="1" max="12" width="14" customWidth="1"/></cols>' if index == 1 else '<cols><col min="1" max="1" width="35" customWidth="1"/></cols>'
    breaks = '<rowBreaks count="1" manualBreakCount="1"><brk id="20" min="0" max="16383" man="1"/></rowBreaks><colBreaks count="1" manualBreakCount="1"><brk id="6" min="0" max="1048575" man="1"/></colBreaks>' if index == 1 else ''
    parts[f'xl/worksheets/sheet{index}.xml'] = f'<worksheet xmlns="{S}"><sheetViews><sheetView workbookViewId="0" tabSelected="{1 if index == 4 else 0}"/></sheetViews><sheetFormatPr defaultRowHeight="24"/>{columns}<sheetData>{rows}</sheetData><printOptions headings="0" gridLines="0"/><pageMargins left="0.5" right="0.5" top="0.5" bottom="0.5" header="0" footer="0"/><pageSetup paperSize="1" orientation="portrait"/>{breaks}</worksheet>'
package(ROOT / 'whole-workbook.xlsx', parts)

NS = 'xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0"'
styles = '<style:style style:name="visible" style:family="table" style:master-page-name="Default"><style:table-properties table:display="true"/></style:style><style:style style:name="hidden" style:family="table" style:master-page-name="Default"><style:table-properties table:display="false"/></style:style><style:style style:name="column" style:family="table-column"><style:table-column-properties style:column-width="1.1in"/></style:style><style:style style:name="row" style:family="table-row"><style:table-row-properties style:row-height="0.3333in" style:use-optimal-row-height="false"/></style:style><style:style style:name="break" style:family="table-row"><style:table-row-properties style:row-height="0.3333in" style:use-optimal-row-height="false" fo:break-before="page"/></style:style>'
for name, color in [('red', 'FF0000'), ('blue', '0000FF'), ('green', '00FF00'), ('yellow', 'FFCC00')]:
    styles += f'<style:style style:name="{name}" style:family="table-cell"><style:table-cell-properties fo:background-color="#{color}"/><style:text-properties fo:font-size="18pt"/></style:style>'


def ods_cell(text, style):
    return f'<table:table-cell table:style-name="{style}" office:value-type="string"><text:p>{text}</text:p></table:table-cell>'


tables = ''
for index, name in enumerate(NAMES, 1):
    area = ' table:print-ranges="\'Wide 宽表\'.A1:\'Wide 宽表\'.D10"' if index == 1 else ''
    tables += f'<table:table table:name="{name}" table:style-name="{"hidden" if index == 2 else "visible"}"{area}><table:table-column table:style-name="column" table:number-columns-repeated="12"/>'
    if index == 1:
        for row in range(1, 35):
            content = ods_cell('VISIBLE FIRST', 'red') if row == 1 else '<table:table-cell table:number-columns-repeated="12"/>'
            if row == 31:
                content = '<table:table-cell table:number-columns-repeated="9"/>' + ods_cell('OUTSIDE PRINT AREA', 'blue')
            tables += f'<table:table-row table:style-name="{"break" if row == 21 else "row"}">{content}</table:table-row>'
    else:
        content = '<table:table-cell/>' if index == 3 else ods_cell('HIDDEN MIDDLE' if index == 2 else 'LAST SHEET', 'green' if index == 2 else 'yellow')
        tables += f'<table:table-row table:style-name="row">{content}</table:table-row>'
    tables += '</table:table>'
package(ROOT / 'whole-workbook.ods', {
    'mimetype': 'application/vnd.oasis.opendocument.spreadsheet',
    'META-INF/manifest.xml': '<manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.2"><manifest:file-entry manifest:full-path="/" manifest:media-type="application/vnd.oasis.opendocument.spreadsheet"/><manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml"/><manifest:file-entry manifest:full-path="styles.xml" manifest:media-type="text/xml"/></manifest:manifest>',
    'content.xml': f'<office:document-content {NS} office:version="1.2"><office:automatic-styles>{styles}</office:automatic-styles><office:body><office:spreadsheet>{tables}</office:spreadsheet></office:body></office:document-content>',
    'styles.xml': f'<office:document-styles {NS} office:version="1.2"><office:styles/><office:automatic-styles><style:page-layout style:name="letter"><style:page-layout-properties fo:page-width="8.5in" fo:page-height="11in" fo:margin="0.5in" style:print-orientation="portrait"/></style:page-layout></office:automatic-styles><office:master-styles><style:master-page style:name="Default" style:page-layout-name="letter"/></office:master-styles></office:document-styles>',
})
files = [ROOT / f'whole-workbook.{ext}' for ext in ['xlsx', 'ods']]
file_records = {p.name: {'bytes': p.stat().st_size, 'sha256': hashlib.sha256(p.read_bytes()).hexdigest()} for p in files}
# XLS is an explicit, separately recorded LibreOffice-derived fixture. Running
# this deterministic XML generator neither invokes an external program nor
# overwrites that binary file or its derivation metadata.
previous = ROOT / 'workbooks-provenance.json'
if previous.exists():
    existing = json.loads(previous.read_text()).get('files', {}).get('whole-workbook.xls')
    binary = ROOT / 'whole-workbook.xls'
    if existing and binary.exists() and hashlib.sha256(binary.read_bytes()).hexdigest() == existing['sha256']:
        file_records['whole-workbook.xls'] = existing
(ROOT / 'workbooks-provenance.json').write_text(json.dumps({
    'provenance': 'Original Markitai Rust fixtures, project license; deterministic standard-library ZIP/XML authoring. No user workbook or third-party input.',
    'files': file_records,
    'source_sheet_order': NAMES,
    'expected': {'first': ['VISIBLE FIRST', 'OUTSIDE PRINT AREA'], 'second': ['HIDDEN MIDDLE'], 'third': 'empty sheet', 'fourth': ['LAST SHEET']},
    'print_settings': 'First sheet has a limited print area, manual row break and wide content; ordinary paper pagination differs from whole-sheet export.',
}, ensure_ascii=False, indent=2) + '\n')
