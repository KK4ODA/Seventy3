"""Builds the Seventy3 Quick Start Guide (docs/Seventy3-Quick-Start.pdf).

Run from anywhere:

    python3 tools/quick-start/make_guide.py

Needs Python 3 with reportlab (`pip install reportlab`) and two free font
families: Carlito and Liberation Mono (on Debian/Ubuntu,
`apt install fonts-crosextra-carlito fonts-liberation`).

The version shown in the guide comes from Cargo.toml. Regenerate the guide
when the app's screens or wording change, and before each minor release;
the Release workflow warns if the guide names a different version.
Keep every on-screen label in this file worded exactly as the app shows it.
"""

import glob
import os
import re
import sys

from reportlab.lib import colors
from reportlab.lib.enums import TA_LEFT
from reportlab.lib.pagesizes import letter
from reportlab.lib.styles import ParagraphStyle
from reportlab.lib.units import inch
from reportlab.pdfbase import pdfmetrics
from reportlab.pdfbase.ttfonts import TTFont
from reportlab.platypus import (
    BaseDocTemplate,
    Flowable,
    Frame,
    Image,
    KeepTogether,
    ListFlowable,
    ListItem,
    PageBreak,
    PageTemplate,
    Paragraph,
    Spacer,
    Table,
    TableStyle,
)

REPO = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
OUT = sys.argv[1] if len(sys.argv) > 1 else os.path.join(REPO, "docs", "Seventy3-Quick-Start.pdf")
SCREENSHOT = sys.argv[2] if len(sys.argv) > 2 else os.path.join(REPO, "docs", "screenshot.png")

with open(os.path.join(REPO, "Cargo.toml"), encoding="utf-8") as f:
    FULL_VERSION = re.search(r'^version\s*=\s*"([^"]+)"', f.read(), re.M).group(1)
# The guide is per minor version: "0.1" covers 0.1.0, 0.1.1, ...
VERSION = ".".join(FULL_VERSION.split(".")[:2])

# What to say about this version's testing status. Update for each release.
STATUS_NOTE = (
    f"<b>Version {VERSION}</b> is the first release and has been tested with Seventy3's built-in simulated "
    "radio. If anything on your station doesn't match this guide, please report it (see the last page)."
)


# ---- fonts and colours -------------------------------------------------------
def font(filename):
    dirs = ["/usr/share/fonts", "/usr/local/share/fonts", os.path.expanduser("~/.fonts"),
            os.path.expanduser("~/.local/share/fonts"), "C:/Windows/Fonts", "/Library/Fonts"]
    for d in dirs:
        hits = glob.glob(os.path.join(d, "**", filename), recursive=True)
        if hits:
            return hits[0]
    sys.exit(f"Font {filename} not found. Install Carlito and Liberation Mono (see the top of this file).")


pdfmetrics.registerFont(TTFont("Body", font("Carlito-Regular.ttf")))
pdfmetrics.registerFont(TTFont("Body-Bold", font("Carlito-Bold.ttf")))
pdfmetrics.registerFont(TTFont("Body-Italic", font("Carlito-Italic.ttf")))
pdfmetrics.registerFont(TTFont("Body-BoldItalic", font("Carlito-BoldItalic.ttf")))
pdfmetrics.registerFont(TTFont("Mono", font("LiberationMono-Regular.ttf")))
pdfmetrics.registerFontFamily("Body", normal="Body", bold="Body-Bold", italic="Body-Italic", boldItalic="Body-BoldItalic")

INK = colors.HexColor("#1B2430")
MUTED = colors.HexColor("#5B6675")
ACCENT = colors.HexColor("#D9831A")  # the app's amber, darkened for print
ACCENT_SOFT = colors.HexColor("#FFF3E0")
RULE = colors.HexColor("#D9DEE5")
TABLE_HEAD = colors.HexColor("#F1F3F6")
OK = colors.HexColor("#1E7F52")
OK_SOFT = colors.HexColor("#E8F5EE")
WARN = colors.HexColor("#B4361F")
WARN_SOFT = colors.HexColor("#FCECE8")

PAGE_W, PAGE_H = letter
MARGIN = 0.75 * inch
CONTENT_W = PAGE_W - 2 * MARGIN

# ---- styles ------------------------------------------------------------------
body = ParagraphStyle("body", fontName="Body", fontSize=11, leading=15, textColor=INK, spaceAfter=5)
small = ParagraphStyle("small", parent=body, fontSize=9.5, leading=12.5, textColor=MUTED, spaceAfter=0)
lead = ParagraphStyle("lead", parent=body, fontSize=12.5, leading=17.5)
title = ParagraphStyle("title", fontName="Body-Bold", fontSize=30, leading=34, textColor=INK)
kicker = ParagraphStyle("kicker", fontName="Body-Bold", fontSize=12, leading=14, textColor=ACCENT)
h2 = ParagraphStyle("h2", fontName="Body-Bold", fontSize=16, leading=20, textColor=INK, spaceBefore=8, spaceAfter=6)
h3 = ParagraphStyle("h3", fontName="Body-Bold", fontSize=12.5, leading=16, textColor=INK, spaceBefore=6, spaceAfter=3)
cell = ParagraphStyle("cell", parent=body, fontSize=10.5, leading=13.5, spaceAfter=0)
cell_head = ParagraphStyle("cell_head", parent=cell, fontName="Body-Bold", textColor=MUTED)
caption = ParagraphStyle("caption", parent=small, alignment=TA_LEFT, spaceBefore=4)


def ui(text):
    """An on-screen label, as it appears in Seventy3."""
    return f"<b>{text}</b>"


def mono(text):
    return f'<font name="Mono" size="9.5">{text}</font>'


SEP = " › "  # menu path separator


def P(text, style=body):
    return Paragraph(text, style)


def bullets(items, numbered=False):
    flow = [ListItem(P(t), leftIndent=16, value=i + 1 if numbered else None) for i, t in enumerate(items)]
    return ListFlowable(
        flow,
        bulletType="1" if numbered else "bullet",
        start="1" if numbered else "•",
        bulletFontName="Body-Bold" if numbered else "Body",
        bulletFontSize=11,
        bulletColor=ACCENT if numbered else MUTED,
        leftIndent=16,
        spaceBefore=0,
        spaceAfter=4,
    )


def callout(label, text, color, fill):
    """A tinted box with a coloured label: Check, If not, Caution..."""
    t = Table(
        [[P(f'<font name="Body-Bold" color="{color.hexval()}">{label}</font>', cell), P(text, cell)]],
        colWidths=[0.95 * inch, CONTENT_W - 0.95 * inch - 0.3 * inch],
    )
    t.setStyle(
        TableStyle(
            [
                ("BACKGROUND", (0, 0), (-1, -1), fill),
                ("LINEBEFORE", (0, 0), (0, -1), 3, color),
                ("VALIGN", (0, 0), (-1, -1), "TOP"),
                ("LEFTPADDING", (0, 0), (-1, -1), 9),
                ("RIGHTPADDING", (0, 0), (-1, -1), 9),
                ("TOPPADDING", (0, 0), (-1, -1), 6),
                ("BOTTOMPADDING", (0, 0), (-1, -1), 7),
            ]
        )
    )
    return [Spacer(1, 3), t, Spacer(1, 4)]


def check(text):
    return callout("Check", text, OK, OK_SOFT)


def if_not(text):
    return callout("If not", text, MUTED, TABLE_HEAD)


def caution(text):
    return callout("Caution", text, WARN, WARN_SOFT)


def note(label, text):
    return callout(label, text, ACCENT, ACCENT_SOFT)


class StepHeader(Flowable):
    """A numbered badge followed by the step's title."""

    def __init__(self, number, text):
        super().__init__()
        self.number, self.text = number, text
        self.height = 30

    def wrap(self, avail_w, avail_h):
        self.width = avail_w
        return avail_w, self.height

    def draw(self):
        c = self.canv
        r = 13
        c.setFillColor(ACCENT)
        c.circle(r, self.height / 2, r, stroke=0, fill=1)
        c.setFillColor(colors.white)
        c.setFont("Body-Bold", 14)
        c.drawCentredString(r, self.height / 2 - 5, str(self.number))
        c.setFillColor(INK)
        c.setFont("Body-Bold", 16)
        c.drawString(2 * r + 10, self.height / 2 - 5.5, self.text)


class Rule(Flowable):
    def __init__(self, color=RULE, thickness=0.8, space=6):
        super().__init__()
        self.color, self.thickness, self.space = color, thickness, space

    def wrap(self, avail_w, avail_h):
        self.width = avail_w
        return avail_w, self.space * 2

    def draw(self):
        self.canv.setStrokeColor(self.color)
        self.canv.setLineWidth(self.thickness)
        self.canv.line(0, self.space, self.width, self.space)


def step(number, heading, *content):
    """A step, kept on one page where possible."""
    parts = [Spacer(1, 6), StepHeader(number, heading), Spacer(1, 4)]
    for c in content:
        parts.extend(c if isinstance(c, list) else [c])
    return KeepTogether(parts)


def grid(rows, widths, head=True):
    data = [[P(c, cell_head if head and i == 0 else cell) for c in row] for i, row in enumerate(rows)]
    t = Table(data, colWidths=widths, repeatRows=1 if head else 0)
    style = [
        ("VALIGN", (0, 0), (-1, -1), "TOP"),
        ("LEFTPADDING", (0, 0), (-1, -1), 8),
        ("RIGHTPADDING", (0, 0), (-1, -1), 8),
        ("TOPPADDING", (0, 0), (-1, -1), 4),
        ("BOTTOMPADDING", (0, 0), (-1, -1), 5),
        ("LINEBELOW", (0, 0), (-1, -1), 0.6, RULE),
    ]
    if head:
        style.append(("BACKGROUND", (0, 0), (-1, 0), TABLE_HEAD))
    t.setStyle(TableStyle(style))
    return t


# ---- page furniture ----------------------------------------------------------
def on_page(c, doc):
    c.saveState()
    y = 0.5 * inch
    c.setStrokeColor(RULE)
    c.setLineWidth(0.6)
    c.line(MARGIN, y + 12, PAGE_W - MARGIN, y + 12)
    c.setFont("Body", 9)
    c.setFillColor(MUTED)
    c.drawString(MARGIN, y, f"Seventy3 Quick Start Guide  ·  version {VERSION}  ·  github.com/KK4ODA/Seventy3")
    c.drawRightString(PAGE_W - MARGIN, y, f"Page {doc.page}")
    c.restoreState()


doc = BaseDocTemplate(
    OUT,
    pagesize=letter,
    leftMargin=MARGIN,
    rightMargin=MARGIN,
    topMargin=0.7 * inch,
    bottomMargin=0.85 * inch,
    title="Seventy3 Quick Start Guide",
    author="Seventy3 project",
    # The Release workflow looks for "Seventy3 <version> with" here.
    subject=f"Setting up Seventy3 {VERSION} with a Kenwood TS-480 and an RTL-SDR",
    keywords="Seventy3, TS-480, RTL-SDR, panadapter, POTA, CAT",
)
frame = Frame(doc.leftMargin, doc.bottomMargin, doc.width, doc.height, id="main", leftPadding=0, rightPadding=0)
doc.addPageTemplates([PageTemplate(id="page", frames=[frame], onPage=on_page)])

story = []

# ---- page 1: what it is, what you need -----------------------------------------
story += [
    P("SEVENTY3", kicker),
    Spacer(1, 2),
    P("Quick Start Guide", title),
    Spacer(1, 6),
    P(
        "Panadapter, waterfall and CAT control for the Kenwood TS-480",
        ParagraphStyle("sub", parent=lead, textColor=MUTED, spaceAfter=10),
    ),
    P(
        "Seventy3 gives a Kenwood TS-480 a live band display. An RTL-SDR dongle connected to the "
        "radio's first IF (73.095 MHz) shows the signals around your frequency as a spectrum and "
        "waterfall. Through the radio's COM port, Seventy3 tunes the radio, changes band and mode, "
        "and keys the transmitter. It also shows Parks on the Air (POTA) activators right on the waterfall.",
        lead,
    ),
    P(STATUS_NOTE, small),
    Spacer(1, 8),
]
img = Image(SCREENSHOT)
img._restrictSize(CONTENT_W, 3.05 * inch)
img.hAlign = "LEFT"
shot = Table([[img]], colWidths=[img.drawWidth])
shot.setStyle(TableStyle([("BOX", (0, 0), (-1, -1), 0.8, RULE), ("LEFTPADDING", (0, 0), (-1, -1), 0),
                          ("RIGHTPADDING", (0, 0), (-1, -1), 0), ("TOPPADDING", (0, 0), (-1, -1), 0),
                          ("BOTTOMPADDING", (0, 0), (-1, -1), 0)]))
shot.hAlign = "LEFT"
story += [
    shot,
    P(
        "Seventy3 in demo mode: spectrum (top), waterfall (below) and POTA spots (right). "
        "The orange line is the radio's frequency.",
        caption,
    ),
    Spacer(1, 8),
    P("What you need", h2),
    bullets(
        [
            "A Kenwood TS-480 with its first IF (73.095 MHz) brought out to a connector for the dongle. "
            "Making this connection is not covered in this guide.",
            "An RTL-SDR USB dongle with an R820T or R828D tuner, connected to that IF connector. "
            "A model with a 1 ppm TCXO drifts less.",
            "A cable from the TS-480's 9-pin COM port to the computer, usually through a USB-to-serial adapter.",
            "A Windows 10 or 11 computer.",
            "A dummy load for the first transmit test.",
        ]
    ),
    *note(
        "IF tap",
        "Feed the dongle through a DC-blocking capacitor and enough isolation (a small buffer amplifier, "
        "or at least a series resistor) that it doesn't load the radio's IF. The tap can carry a strong "
        "signal while you transmit.",
    ),
    PageBreak(),
]

# ---- setup steps -------------------------------------------------------------------
story += [P("Setting up", h2), P("Do these steps in order. Steps 1 to 3 need no radio.", body)]

story.append(
    step(
        1,
        "Download Seventy3",
        bullets(
            [
                f"In a web browser, go to {ui('github.com/KK4ODA/Seventy3/releases/latest')}.",
                f"Under {ui('Assets')}, click {ui('seventy3.exe')} to download it.",
                f"Make a folder you can write to, for example {mono('Documents\\Seventy3')}, and move "
                f"{ui('seventy3.exe')} into it.",
            ],
            numbered=True,
        ),
        P(
            "There is nothing to install. Seventy3 updates itself by replacing this file, so don't put "
            "it in Program Files, where it can't.",
            small,
        ),
    )
)

story.append(
    step(
        2,
        "Start it and try the demo",
        bullets(
            [
                f"Double-click {ui('seventy3.exe')}.",
                f"If Windows shows {ui('Windows protected your PC')}, click {ui('More info')}, then "
                f"{ui('Run anyway')}. Windows shows this for programs from small publishers.",
                f"On the welcome screen, click {ui('Try the demo')}. This uses a simulated radio and band, "
                "so no hardware is needed yet.",
            ],
            numbered=True,
        ),
        check(
            f"The waterfall scrolls, POTA spots are listed on the right, and the toolbar shows {ui('DEMO')}. "
            "Click a signal or a spot to see Seventy3 tune to it."
        ),
        if_not(
            f"If the window doesn't open or stays blank, right-click {ui('seventy3.exe')} and create a shortcut. "
            f"Open the shortcut's {ui('Properties')}, add a space and {mono('--opengl')} at the end of "
            f"{ui('Target')}, and start Seventy3 from the shortcut."
        ),
    )
)

story.append(
    step(
        3,
        "Install the RTL-SDR driver (once)",
        P(
            "Windows needs the WinUSB driver to talk to the dongle. Skip this step if another SDR program "
            "(SDR#, SDR++ or HDSDR) already works with your dongle."
        ),
        bullets(
            [
                "Plug the RTL-SDR into a USB port.",
                f"Download Zadig from {ui('zadig.akeo.ie')} and run it.",
                f"Click {ui('Options' + SEP + 'List All Devices')}.",
                f"In the drop-down list, choose {ui('Bulk-In, Interface (Interface 0)')}.",
                f"Check that the driver to the right of the green arrow is {ui('WinUSB')}, then click "
                f"{ui('Replace Driver')} (or {ui('Install Driver')}).",
            ],
            numbered=True,
        ),
        caution(
            "Make sure the selected device is the dongle. Replacing the driver of another device, "
            "such as your keyboard or mouse, can stop it working."
        ),
    )
)

story.append(
    step(
        4,
        "Connect the radio",
        bullets(
            [
                "On the TS-480, set the COM port speed in the menu (menu No. 56; see the TS-480 manual). "
                "9600 is a good choice.",
                "Connect the CAT cable and the RTL-SDR, and turn the radio on.",
                f"In Seventy3, click {ui('Settings')} (right end of the toolbar), then the {ui('Rig (CAT)')} tab.",
                f"Untick {ui('Demo mode')}.",
                f"Choose the {ui('CAT port')} (the COM number of your serial adapter) and set {ui('Baud rate')} "
                "to the same speed as the radio.",
                "Close the Settings window.",
            ],
            numbered=True,
        ),
        check(
            f"The bar along the bottom shows a green dot and {ui('TS-480 on COM… at … baud')}, and the "
            "frequency display follows the radio's tuning knob. Next to it, the RTL-SDR is listed with a "
            "green dot, and the waterfall fills with the band."
        ),
        if_not("See Troubleshooting on the last page."),
    )
)

story.append(
    step(
        5,
        "Check the waterfall direction",
        P("Turn the radio's tuning knob up a few kHz. Signals on the waterfall should slide to the <b>left</b>."),
        if_not(
            f"If they slide to the right, open {ui('Settings' + SEP + 'IF receiver')} and untick "
            f"{ui('IF is mirrored')}."
        ),
    )
)

story.append(
    step(
        6,
        "Calibrate the frequency",
        P(
            "The dongle's crystal is never exactly on frequency, so signals can appear slightly off. "
            "Calibrate once, and again if spot markers don't line up with their signals."
        ),
        bullets(
            [
                "Tune the radio, in AM mode, to a station with a steady carrier on a known frequency: "
                "for example WWV on 5000, 10000 or 15000 kHz, or CHU on 3330, 7850 or 14670 kHz.",
                f"Click {ui('Calibrate')} in the toolbar. In the window that opens, check that "
                f"{ui('Its true frequency')} shows the station's frequency in kHz.",
                f"Zoom in with the {ui('+')} button next to {ui('Span')} until the carrier is a thin, clear line.",
                "Click exactly on the center of the carrier.",
            ],
            numbered=True,
        ),
        check("A message at the bottom right confirms the calibration."),
    )
)

story.append(
    step(
        7,
        "Set the RF gain",
        P(f"Open {ui('Settings' + SEP + 'IF receiver')} and adjust {ui('RF gain')}. About 20 dB is a good start."),
        bullets(
            [
                f"If {ui('ADC overload: lower the RF gain')} appears in the bottom bar, lower the gain "
                "until it goes away.",
                "If signals you can hear on the radio don't show on the waterfall, raise it.",
            ]
        ),
    )
)

story.append(
    step(
        8,
        "Test transmit into a dummy load",
        bullets(
            [
                "Connect a dummy load to the radio's antenna connector.",
                f"Click {ui('PTT')} at the top right. The button turns red and shows {ui('ON AIR')}, "
                "and the radio transmits.",
                f"Press {ui('Esc')}. The radio returns to receive.",
                f"Now hold the {ui('Space')} bar to transmit, and let go to stop.",
            ],
            numbered=True,
        ),
        note(
            "Safety",
            f"{ui('Esc')} always stops transmitting. If the radio is left transmitting, Seventy3 stops it "
            f"after 3 minutes (change this in {ui('Settings' + SEP + 'Rig (CAT)' + SEP + 'Transmit timeout')}). "
            "Closing Seventy3 always returns the radio to receive.",
        ),
    )
)

story += [Spacer(1, 6), P("<b>That's it: you're ready to operate.</b> The rest of this guide is a reference for everyday use.", body)]

# ---- reference ---------------------------------------------------------------------
W1 = 1.75 * inch
everyday = [
    P("Everyday use", h2),
    grid(
        [
            ["To", "Do this"],
            ["Tune to a signal", "Click it on the waterfall. In SSB, the signal lands in your receive passband."],
            ["Fine-tune", f"Turn the mouse wheel, or press {ui('←')} and {ui('→')}. Hold {ui('Shift')} for 10× steps."],
            ["Slide along the band", "Drag the waterfall left or right."],
            ["Zoom in or out", f"The {ui('+')} and {ui('−')} buttons next to {ui('Span')}, the {ui('+')} and {ui('−')} "
             f"keys, or {ui('Ctrl')} + mouse wheel."],
            ["Type a frequency", f"Double-click the frequency display, type {mono('14285')} (kHz) or {mono('14.285')} "
             f"(MHz), and press {ui('Enter')}."],
            ["Change one digit", "Point at the digit in the frequency display and turn the mouse wheel."],
            ["Change band or mode", "Use the buttons at the top. Each band remembers where you left it."],
            ["Work a POTA activator", "Click the spot in the list, or its label on the waterfall. "
             "Seventy3 sets the frequency and mode."],
            ["Transmit", f"Hold {ui('Space')}, or click {ui('PTT')}. {ui('Esc')} always stops."],
        ],
        [W1, CONTENT_W - W1],
    ),
]
story += [
    KeepTogether(everyday),
    Spacer(1, 4),
    KeepTogether([P("POTA spots", h3), P(
        f"The list on the right refreshes every minute. {ui('SSB')}, {ui('CW')} and {ui('Data')} choose which "
        f"modes to show. {ui('Band')} shows only the band you're on. {ui('No QRT')} hides activators who have "
        f"signed off. The sort button switches between newest first and by frequency. {ui('Spots')} in the "
        "toolbar hides or shows the list."
    )]),
    KeepTogether([P("Updates", h3), P(
        f"Seventy3 looks for a new version when it starts and once a day. When there is one, a bar at the top "
        f"offers {ui('Update and restart')}. Updates are digitally signed: Seventy3 checks the signature and "
        "the downloaded file before installing, and never restarts while you're transmitting. To turn off "
        f"automatic checks, or to check now, open {ui('Settings' + SEP + 'About')}."
    )]),
    P("Troubleshooting", h2),
    grid(
        [
            ["Message or problem", "What to do"],
            [ui("no CAT port selected"), f"Choose the {ui('CAT port')} in {ui('Settings' + SEP + 'Rig (CAT)')}."],
            [ui("Can't open CAT port"), "The COM number is wrong, or another program (a logger, WSJT-X, OmniRig) "
             "is using the port. Close that program; Seventy3 retries every few seconds."],
            [f"{ui('Waiting for the rig to answer')} or {ui('Rig not answering')}",
             f"Check the radio is on, the cable is connected, and {ui('Baud rate')} matches the radio's menu. "
             f"Try ticking {ui('RTS on')} under {ui('Control lines')}."],
            [ui("Unrecognized data from the rig"), f"{ui('Baud rate')} doesn't match the radio's COM speed."],
            ["The radio transmits when Seventy3 connects", f"Untick {ui('RTS on')} and {ui('DTR on')} under "
             f"{ui('Settings' + SEP + 'Rig (CAT)' + SEP + 'Control lines')}."],
            ["RTL-SDR error mentioning Zadig", "Do step 3. Try another USB port."],
            ["Waterfall blank, or all one color", "Check the cable from the IF tap, then the RF gain (step 7)."],
            ["Signals move the wrong way when tuning", "See step 5."],
            ["Spot labels don't line up with signals", "Calibrate (step 6)."],
            [ui("ADC overload"), "Lower the RF gain, or add an attenuator at the IF tap."],
            ["The window doesn't open", f"Start Seventy3 with {mono('--opengl')} (step 2)."],
        ],
        [2.35 * inch, CONTENT_W - 2.35 * inch],
    ),
    Spacer(1, 4),
    KeepTogether([P("Getting help", h3), P(
        f"Report problems at {ui('github.com/KK4ODA/Seventy3/issues')}. Please attach the log file. To find it, "
        f"paste {mono('%LOCALAPPDATA%\\Seventy3\\data')} into the File Explorer address bar; the file is "
        f"{mono('seventy3.log')}. Settings are kept in {mono('%APPDATA%\\Seventy3\\config')}."
    )]),
    Rule(space=4),
    P(
        "Seventy3 is free software under the GNU General Public License, version 3 or later, and comes with "
        "no warranty. Source code: github.com/KK4ODA/Seventy3",
        small,
    ),
]

doc.build(story)
print("wrote", OUT)
