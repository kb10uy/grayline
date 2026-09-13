app-title = Grayline RTTY

menu-file = File
menu-view = View
menu-settings = Settings
menu-help = Help
menu-zoom-in = Zoom In
menu-zoom-out = Zoom Out
menu-zoom-reset = Reset Zoom ({ $percent }%)
menu-open-config = Open Config Folder
menu-quit = Quit
menu-language = Language
menu-station = My Station...
menu-manual = Manual

input-device = Input device
output-device = Output device

section-tuning = Tuning
section-transmit = Transmit

label-mark = Mark
label-shift = Shift
label-speed = Speed
label-signal = Signal

action-reverse = Rev
action-afc = AFC
action-squelch = Squelch
action-adopt-tones = Take Detected Pair
action-clear = Clear
action-resync = Resync
action-unshift-on-space = Unshift on Space
action-atc = Threshold Correction

path-resonator = IIR + majority

case-letters = LTRS
case-figures = FIGS

status-audio = { $rate } Hz
status-no-audio = No input device
status-no-output = No output device
status-dropped = { $samples } samples dropped
status-reading = Reading

hint-listening = Nothing printed yet. Tune a receiver in, or drop a WAV recording here.
hint-resync = Start the framing again, for a line that has lost its place

error-config = Configuration could not be read
error-device-lost = The capture device stopped
error-open-folder = The folder could not be opened
error-open-manual = The manual could not be opened in a browser
error-wav = The recording could not be read

label-on-air = Sending
label-queued = Queued
label-level = Level
label-templates = Templates

action-send = Send
action-stop = Stop

status-remaining = { $seconds } s left

hint-squelch = Nothing prints until the signal reads above this. Zero turns it off.
hint-stop = Stop sending and put whatever did not go out back in the message
hint-drop-queued = Drop this message from the queue
hint-templates = Write one of the messages kept in templates.toml into the field. Ctrl+F1 to Ctrl+F9 pick the first nine.
hint-unsendable = { $character } has no Baudot code. Remove it before sending.

error-no-output = No output device is selected
error-underrun = The sound card ran out of audio; the message that went out has a gap in it

section-contact = Contact

label-my-call = My call
label-my-name = My name
label-my-qth = My QTH
label-my-grid = My grid
label-his-call = His call
label-his-name = His name
label-his-qth = His QTH
label-rst-sent = RST sent
label-rst-received = RST rcvd

action-clear-contact = Clear Contact

hint-clear-contact = Empty the contact fields, ready for the next station

station-title = My Station
station-close = Close
station-callsign-required = The callsign is what every macro signs with. The name and QTH fill in the macros that mention them.

custom-title = Extra fields
custom-name = name
custom-value = value
custom-add = Add field
custom-invalid = A name may hold letters, digits, and underscores, in dot-separated parts starting with a letter
custom-note = A field named here is written in a macro as ${ "{" }custom.name{ "}" }. Its value goes on the air, so it has to be sendable.

menu-contact = Contact Directory
action-contact-lookup = Look Up Contacts
action-contact-write-credentials = Write credentials.toml

contact-title = Contact
contact-open = What the directory has filed under this station
contact-note-keys = A macro reads these as ${ "{" }contact.name{ "}" }. Anything you type here is yours; no lookup will overwrite it.
contact-other = Other Fields
contact-add = Add
contact-refresh = Look Up Again
contact-state-looking = Looking up…
contact-state-unknown = Nothing is filed under { $callsign }.
contact-state-failed = The lookup failed: { $detail }
contact-credentials-written = Wrote { $path }
contact-name = Name
contact-name-latin = Name (Latin)
contact-qth = QTH
contact-qth-latin = QTH (Latin)
contact-grid = Grid
contact-jcc = JCC/JCG
contact-dxcc = DXCC
contact-dxcc-id = DXCC No.
contact-cq-zone = CQ Zone
contact-itu-zone = ITU Zone
contact-continent = Continent
contact-state = State
contact-county = County
contact-iota = IOTA
contact-qsl-manager = QSL Via
contact-email = Email
contact-note = Note

error-credentials = The credentials file could not be written
