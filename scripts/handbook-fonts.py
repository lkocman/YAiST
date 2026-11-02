#!/usr/bin/python3
"""Static fonts for the handbook PDF, written to the directory given.

asciidoctor-pdf (prawn) uses only the default instance of a variable font,
so bold and italic come out regular. Inter comes from the distribution's
inter-fonts collection, which has real italics; Archivo and Geist Mono are
instantiated from the variable fonts YAiST bundles (assets/fonts).
"""
import sys
from pathlib import Path

from fontTools.ttLib import TTCollection, TTFont
from fontTools.varLib.instancer import instantiateVariableFont

out = Path(sys.argv[1])
out.mkdir(parents=True, exist_ok=True)
assets = Path(__file__).resolve().parent.parent / "assets" / "fonts"

inter = {"Inter Regular": "regular", "Inter Bold": "bold", "Inter Italic": "italic", "Inter Bold Italic": "bold_italic"}
for font in TTCollection("/usr/share/fonts/truetype/Inter.ttc").fonts:
    style = inter.get(font["name"].getDebugName(4))
    if style:
        font.save(out / f"Inter-{style}.ttf")

for family, src in (("Archivo", "Archivo-Variable.ttf"), ("GeistMono", "GeistMono-Variable.ttf")):
    for style, weight in (("regular", 400), ("bold", 700)):
        font = TTFont(assets / src)
        axes = {a.axisTag: a.defaultValue for a in font["fvar"].axes}
        axes["wght"] = weight
        instantiateVariableFont(font, axes, inplace=True, updateFontNames=False)
        font.save(out / f"{family}-{style}.ttf")
