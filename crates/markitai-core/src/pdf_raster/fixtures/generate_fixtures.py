"""Author PDF raster/OCR fixtures using only Python's standard library.

The existing OCR fixture's unchanged PNG IDAT payload is embedded directly with
PDF's PNG predictor. No bitmap is generated, decoded, edited, or rendered here.
"""
from pathlib import Path
import argparse
import hashlib
import json
import math
import struct
import zlib

ROOT = Path(__file__).resolve().parents[5]
PNG = ROOT / 'crates/markitai-core/src/ocr/fixtures/english.png'
TRANSCRIPT = PNG.with_suffix('.txt')
EXPECTED_PNG = 'a9f547051baeaaa96c9299363a3c79b1fb3842c2c7192c0bde0663f86081b94d'


def digest(data):
    return hashlib.sha256(data).hexdigest()


def load_png():
    data = PNG.read_bytes()
    assert digest(data) == EXPECTED_PNG
    assert data[:8] == b'\x89PNG\r\n\x1a\n'
    position = 8
    payloads = []
    header = None
    ended = False
    while position < len(data):
        length = struct.unpack('>I', data[position:position + 4])[0]
        tag = data[position + 4:position + 8]
        value = data[position + 8:position + 8 + length]
        checksum = struct.unpack('>I', data[position + 8 + length:position + 12 + length])[0]
        assert zlib.crc32(tag + value) == checksum
        if tag == b'IHDR':
            header = struct.unpack('>IIBBBBB', value)
        if tag == b'IDAT':
            payloads.append(value)
        position += length + 12
        if tag == b'IEND':
            ended = True
            break
    assert ended and position == len(data)
    width, height, bits, color, compression, filtering, interlace = header
    assert (bits, color, compression, filtering, interlace) == (8, 0, 0, 0, 0)
    payload = b''.join(payloads)
    assert len(zlib.decompress(payload)) == (width + 1) * height
    return width, height, payload, data


class Pdf:
    def __init__(self):
        self.objects = []
        self.tree = self.reserve()
        self.font = self.add(b'<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>')
        self.pages = []

    def reserve(self):
        self.objects.append(None)
        return len(self.objects)

    def add(self, value):
        index = self.reserve()
        self.objects[index - 1] = value
        return index

    def stream(self, value, entries='', compress=True):
        if compress:
            value = zlib.compress(value, 9)
            entries += ' /Filter /FlateDecode'
        return self.add(f'<< /Length {len(value)} {entries} >>\nstream\n'.encode() + value + b'\nendstream')

    def image(self):
        width, height, payload, _ = load_png()
        return self.stream(payload, f'/Type /XObject /Subtype /Image /Width {width} /Height {height} /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /FlateDecode /DecodeParms << /Predictor 15 /Colors 1 /BitsPerComponent 8 /Columns {width} >>', compress=False)

    def page(self, content, resources, media=(0, 0, 612, 792), crop=None, rotate=0):
        stream = self.stream(content.encode('ascii'))
        extra = f'/Rotate {rotate}'
        if crop:
            extra += ' /CropBox [' + ' '.join(map(str, crop)) + ']'
        page = self.add(f'<< /Type /Page /Parent {self.tree} 0 R /MediaBox [{" ".join(map(str, media))}] /Contents {stream} 0 R /Resources {resources} {extra} >>'.encode())
        self.pages.append(page)

    def finish(self):
        children = ' '.join(f'{page} 0 R' for page in self.pages)
        self.objects[self.tree - 1] = f'<< /Type /Pages /Kids [{children}] /Count {len(self.pages)} >>'.encode()
        catalog = self.add(f'<< /Type /Catalog /Pages {self.tree} 0 R >>'.encode())
        output = bytearray(b'%PDF-1.7\n%\xe2\xe3\xcf\xd3\n')
        offsets = [0]
        for number, value in enumerate(self.objects, 1):
            assert value is not None
            offsets.append(len(output))
            output += f'{number} 0 obj\n'.encode() + value + b'\nendobj\n'
        startxref = len(output)
        output += f'xref\n0 {len(offsets)}\n0000000000 65535 f \n'.encode()
        for offset in offsets[1:]:
            output += f'{offset:010d} 00000 n \n'.encode()
        output += f'trailer\n<< /Size {len(offsets)} /Root {catalog} 0 R >>\nstartxref\n{startxref}\n%%EOF\n'.encode()
        return bytes(output)


def text(value, x, y, size=12):
    escaped = value.replace('\\', '\\\\').replace('(', '\\(').replace(')', '\\)')
    return f'BT /F1 {size} Tf 1 0 0 1 {x} {y} Tm ({escaped}) Tj ET\n'


def font_resources(pdf, extras=''):
    return f'<< /Font << /F1 {pdf.font} 0 R >> {extras} >>'


def image_content(width, height, x, y):
    return f'q {width:g} 0 0 {height:g} {x:g} {y:g} cm /Scan Do Q\n'


def frame(width, height):
    return {'display_points': [width, height], 'dpi': 150, 'pixel_size_ceil': [math.ceil(width * 150 / 72), math.ceil(height * 150 / 72)], 'pixel_origin': 'top-left', 'pixel_background': 'opaque white'}


def mixed():
    pdf = Pdf()
    sentences = [
        'Native page retains its structured text.',
        'Only the scanned page needs local recognition.',
        'Every paragraph remains in its original page order.',
        'A blank page retains its marker and an explicit OCR outcome.',
    ]
    content = text('NATIVE PAGE ONE', 48, 738, 20)
    content += ''.join(text(line, 48, 680 - index * 42, 12) for index, line in enumerate(sentences))
    pdf.page(content, font_resources(pdf))
    image = pdf.image()
    width, height, _, _ = load_png()
    pdf.page(image_content(540, 540 * height / width, 36, 470), f'<< /XObject << /Scan {image} 0 R >> >>')
    pdf.page('', '<< >>')
    expectations = [
        {'page': 1, 'kind': 'native', 'expected_text_contains': ['NATIVE PAGE ONE', *sentences], 'per_page_routing': 'retain current native Markdown exactly; no OCR call', **frame(612, 792)},
        {'page': 2, 'kind': 'scanned', 'expected_native_text': '', 'expected_ocr_whitespace_tokens': TRANSCRIPT.read_text().split(), 'per_page_routing': 'render and OCR this page', **frame(612, 792)},
        {'page': 3, 'kind': 'blank', 'expected_native_text': '', 'expected_ocr_text': '', 'per_page_routing': 'empty native page may enter OCR; empty recognized output is explicitly recorded, with no invented text', **frame(612, 792)},
    ]
    return pdf.finish(), {'pages': expectations, 'document': {'page_count': 3, 'markers': [f'<!-- Page number: {page} -->' for page in range(1, 4)], 'screenshot_pages_when_enabled': [1, 2, 3], 'expected_ocr_attempt_pages': [2, 3], 'expected_nonempty_ocr_pages': [2], 'requires_native_page_output_comparison_to_same_source_without_ocr': True}}


def rotated(rotation):
    pdf = Pdf()
    image = pdf.image()
    matrices = {0: (1, 0, 0, 1, 40, 60), 90: (0, 1, -1, 0, 640, 60), 180: (-1, 0, 0, -1, 640, 860), 270: (0, -1, 1, 0, 40, 860)}
    width, height = (800, 600) if rotation in (90, 270) else (600, 800)
    # Contents are counter-rotated in page space, so correct /Rotate handling
    # presents upright text and the same independently named corner colors.
    content = 'q ' + ' '.join(map(str, matrices[rotation])) + ' cm\n'
    corners = [('top_left', 20, height - 50, (1, 0, 0)), ('top_right', width - 50, height - 50, (0, 1, 0)), ('bottom_left', 20, 20, (0, 0, 1)), ('bottom_right', width - 50, 20, (0, 0, 0))]
    for _, x, y, color in corners:
        content += f'{color[0]} {color[1]} {color[2]} rg {x} {y} 30 30 re f\n'
    header = f'ROTATION {rotation} CROP CHECK'
    content += '0 g\n' + text(header, 70, height - 92, 16)
    iw, ih, _, _ = load_png()
    dw = min(500, width - 140)
    dh = dw * ih / iw
    content += image_content(dw, dh, (width - dw) / 2, (height - dh) / 2)
    content += 'Q\n' + text('OUTSIDE CROP MUST NOT BE RASTERIZED', 5, 10, 8)
    pdf.page(content, font_resources(pdf, f'/XObject << /Scan {image} 0 R >>'), media=(0, 0, 700, 900), crop=(40, 60, 640, 860), rotate=rotation)
    checks = []
    for name, x, y, color in corners:
        checks.append({'corner': name, 'center_display_points_top_left': [x + 15, height - y - 15], 'expected_rgb': [v * 255 for v in color], 'channel_tolerance': 4})
    return pdf.finish(), {'pages': [{'page': 1, 'kind': 'rotation_crop', 'media_box': [0, 0, 700, 900], 'crop_box': [40, 60, 640, 860], 'rotate': rotation, 'content_counter_rotation_matrix': list(matrices[rotation]), 'expected_ocr_text_contains': [header, *TRANSCRIPT.read_text().splitlines()], 'expected_ocr_text_absent': ['OUTSIDE CROP MUST NOT BE RASTERIZED'], 'corner_pixel_checks': checks, 'routing_expectation': 'geometry fixture; no native reliability verdict prescribed. Force OCR for the independent raster/OCR test.', **frame(width, height)}]}


def form_alpha():
    pdf = Pdf()
    visible = '0 0.7 0 rg 30 30 70 50 re f\n0 0 1 rg 130 30 m 210 30 l 170 100 l h f\n0 g\n' + text('FORM VISIBLE TEXT', 30, 150, 18)
    inner = pdf.stream(visible.encode(), f'/Type /XObject /Subtype /Form /FormType 1 /BBox [0 0 280 210] /Resources {font_resources(pdf)}')
    outer = pdf.stream(b'q 1 0 0 1 60 120 cm /Inner Do Q\n', f'/Type /XObject /Subtype /Form /FormType 1 /BBox [0 0 500 400] /Resources << /XObject << /Inner {inner} 0 R >> >>')
    invisible = pdf.add(b'<< /Type /ExtGState /ca 0 /CA 0 >>')
    resources = font_resources(pdf, f'/XObject << /Outer {outer} 0 R >> /ExtGState << /Invisible {invisible} 0 R >>')
    content = '/Outer Do\nq /Invisible gs\n' + text('ALPHA HIDDEN SENTINEL', 50, 345, 22) + 'Q\n'
    pdf.page(content, resources, media=(0, 0, 500, 400))
    return pdf.finish(), {'pages': [{'page': 1, 'kind': 'nested_form_vector_alpha', 'expected_ocr_text_contains': ['FORM VISIBLE TEXT'], 'expected_ocr_text_absent': ['ALPHA HIDDEN SENTINEL'], 'pixel_checks': [{'center_display_points_top_left': [125, 225], 'expected_rgb': [0, 179, 0], 'channel_tolerance': 4}, {'center_display_points_top_left': [230, 220], 'expected_rgb': [0, 0, 255], 'channel_tolerance': 4}], 'scope': 'nested Form transforms, vector fills and zero alpha. This fixture does not claim to exercise a soft/stencil image mask.', **frame(500, 400)}]}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--out', type=Path, required=True)
    options = parser.parse_args()
    options.out.mkdir(parents=True, exist_ok=False)
    _, _, idat, png = load_png()
    authored = [('mixed-native-scanned-blank.pdf', *mixed())]
    authored += [(f'rotate-crop-{rotation}.pdf', *rotated(rotation)) for rotation in (0, 90, 180, 270)]
    authored += [('nested-form-vector-alpha.pdf', *form_alpha())]
    manifest = {'schema': 1, 'status': 'authored fixtures and independent expectations; native raster/OCR acceptance has not run', 'generator': {'path': str(Path(__file__).resolve()), 'sha256': digest(Path(__file__).read_bytes())}, 'dependencies': 'Python standard library only; no Cargo, converter, OCR, renderer, network, or image editing tool invoked', 'zlib_version': zlib.ZLIB_VERSION, 'source_image': {'path': str(PNG), 'sha256': digest(png), 'bytes': len(png), 'idat_payload_sha256': digest(idat), 'encoding': 'unchanged grayscale PNG IDAT as FlateDecode + PNG Predictor 15'}, 'transcript': {'path': str(TRANSCRIPT), 'sha256': digest(TRANSCRIPT.read_bytes())}, 'fixtures': []}
    for name, data, expectations in authored:
        path = options.out / name
        with path.open('xb') as file:
            file.write(data)
        manifest['fixtures'].append({'name': name, 'bytes': len(data), 'sha256': digest(data), **expectations})
    with (options.out / 'manifest.json').open('x') as file:
        json.dump(manifest, file, indent=2)
        file.write('\n')
    print(json.dumps({'directory': str(options.out), 'fixtures': len(authored), 'total_pdf_bytes': sum(len(data) for _, data, _ in authored), 'manifest_sha256': digest((options.out / 'manifest.json').read_bytes())}))


if __name__ == '__main__':
    main()
