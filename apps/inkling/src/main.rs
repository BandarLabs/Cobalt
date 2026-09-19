//! Inkling: a deterministic, offline five-letter daily puzzle.
use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::{action_id, ActionId, Context, Glyph, KoboApp, Screen, ScreenBuilder, StoreResult};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};
const STATE: &str = "inkling-state-v1";
const EXPORT: &str = "export-result.txt";
const SALT: &str = "inkling-offline-2026";
const ARCHIVE_DAYS: i64 = 365;
const ARCHIVE_PAGE: usize = 5;
const ANSWERS: &[&str] = &[
    "crane", "stare", "piano", "flint", "woven", "mirth", "caper", "bloom", "quiet", "ridge",
    "slope", "charm", "about", "above", "abuse", "actor", "acute", "admit", "adopt", "adult",
    "after", "again", "agent", "agree", "ahead", "alarm", "album", "alike", "alive", "allow",
    "alloy", "alley", "alpha", "altar", "amend", "angel", "anger", "angle", "ankle", "annex",
    "apron", "arena", "argue", "arise", "armor", "aroma", "array", "arrow", "aside", "asset",
    "atlas", "attic", "audio", "audit", "avail", "avoid", "await", "awake", "award", "aware",
    "awful", "bacon", "badge", "badly", "bagel", "baker", "balmy", "banjo", "basic", "basil",
    "basin", "basis", "batch", "baton", "bayou", "begun", "belly", "below", "bench", "berth",
    "bevel", "bible", "birth", "blend", "bless", "blind", "blink", "bliss", "blitz", "block",
    "blond", "blood", "blush", "board", "boast", "bonus", "boost", "booth", "bound", "brain",
    "brand", "brave", "bravo", "brief", "brine", "brink", "brisk", "broad", "broke", "brook",
    "broom", "brush", "buddy", "build", "bunch", "burst", "buyer", "cabin", "cable", "camel",
    "canal", "candy", "canoe", "cargo", "carry", "carve", "catch", "cause", "cedar", "chain",
    "chalk", "chant", "chaos", "chart", "cheap", "check", "cheer", "chess", "chest", "chief",
    "child", "choir", "chord", "civic", "civil", "claim", "clasp", "class", "cliff", "cloak",
    "close", "cloth", "coach", "coast", "colon", "color", "comet", "count", "court", "cover",
    "crack", "craft", "crash", "crate", "crawl", "cream", "crest", "crime", "crisp", "cross",
    "crowd", "crown", "cruel", "crush", "crust", "curry", "curve", "cycle", "daily", "dairy",
    "daisy", "dealt", "debut", "decay", "delay", "delta", "dense", "depth", "diary", "digit",
    "dirty", "ditch", "dizzy", "dodge", "doing", "donor", "doubt", "dough", "draft", "drain",
    "drama", "drank", "drape", "drawl", "dread", "dress", "dried", "drift", "drill", "drink",
    "drive", "drone", "droop", "drove", "dryly", "dusty", "dwell", "eager", "eagle", "early",
    "easel", "eaten", "elder", "elect", "elite", "elbow", "empty", "enact", "enemy", "enjoy",
    "enter", "entry", "equal", "equip", "erase", "error", "essay", "event", "every", "exact",
    "exile", "exist", "extra", "fable", "faint", "fairy", "faith", "fancy", "fatal", "fault",
    "favor", "feast", "fence", "ferry", "fever", "fiber", "fifth", "fifty", "fight", "final",
    "first", "flair", "flask", "fleet", "flesh", "flick", "float", "flock", "flood", "floor",
    "flour", "fluid", "flush", "focal", "focus", "foggy", "force", "forge", "forth", "forty",
    "forum", "found", "frame", "frank", "fraud", "fried", "frock", "frost", "froth", "fruit",
    "fully", "funny", "gauge", "genre", "ghost", "given", "gland", "glare", "glide", "gloom",
    "glory", "gloss", "glove", "grace", "grade", "grain", "grand", "grant", "graph", "grasp",
    "grass", "grave", "gravy", "great", "greet", "grief", "grill", "grind", "groan", "groom",
    "gross", "group", "grove", "growl", "grown", "guard", "guess", "guest", "guide", "guilt",
    "habit", "handy", "happy", "harsh", "haste", "hatch", "haunt", "haven", "hazel", "heavy",
    "hedge", "hello", "hence", "hobby", "hoist", "honey", "honor", "horse", "hotel", "hover",
    "human", "humid", "humor", "hurry", "ideal", "image", "imply", "inbox", "index", "inner",
    "input", "issue", "jelly", "jewel", "joint", "joker", "judge", "juice", "karma", "kayak",
    "keeps", "khaki", "kiosk", "kitty", "knife", "knock", "known", "label", "labor", "laden",
    "lance", "large", "laser", "later", "laugh", "layer", "learn", "lease", "least", "leave",
    "legal", "level", "lever", "lilac", "limit", "linen", "liner", "liver", "lobby", "local",
    "lodge", "logic", "loose", "lover", "lower", "loyal", "lucky", "lunch", "lyric", "magic",
    "major", "maker", "manor", "march", "marry", "match", "mayor", "meant", "medal", "media",
    "melon", "mercy", "merge", "merit", "merry", "messy", "meter", "midst", "might", "mimic",
    "minor", "minus", "model", "modem", "moist", "money", "month", "moral", "motel", "motor",
    "motto", "mound", "mount", "mourn", "mouse", "mouth", "movie", "muddy", "mural", "music",
    "naive", "nasty", "naval", "nerve", "never", "newly", "noble", "noise", "north", "noted",
    "novel", "nurse", "nylon", "oasis", "occur", "offer", "often", "onion", "opera", "orbit",
    "order", "organ", "other", "outer", "owner", "oxide", "ozone", "paint", "panel", "panic",
    "paper", "party", "pasta", "paste", "patch", "patio", "pause", "peace", "peach", "pedal",
    "penny", "perch", "phase", "phone", "photo", "piece", "pilot", "pinch", "pitch", "pivot",
    "pixel", "pizza", "place", "plain", "plane", "plate", "plaza", "plead", "point", "polar",
    "porch", "pound", "power", "press", "price", "pride", "prime", "print", "prior", "prize",
    "probe", "prone", "proof", "prose", "prove", "proxy", "pulse", "punch", "pupil", "purse",
    "queen", "query", "quest", "queue", "quick", "quilt", "quota", "quote", "radar", "radio",
    "rainy", "raise", "rally", "ranch", "range", "rapid", "ratio", "reach", "react", "ready",
    "realm", "rebel", "refer", "reign", "relax", "relay", "renew", "reply", "reset", "resin",
    "rhyme", "rigid", "rinse", "rival", "robin", "robot", "rocky", "rogue", "roman", "roost",
    "rotor", "rough", "route", "royal", "ruler", "rumor", "rural", "rusty", "salad", "saint",
    "salon", "salsa", "sandy", "sauce", "scale", "scare", "scarf", "scene", "scent", "scope",
    "score", "scout", "scrap", "screw", "sedan", "sense", "serve", "seven", "shade", "shake",
    "shale", "shall", "shame", "shape", "share", "sharp", "shave", "sheep", "sheer", "sheet",
    "shelf", "shell", "shift", "shirt", "shock", "shoot", "short", "shout", "shown", "sight",
    "silly", "since", "sixth", "sixty", "skate", "skill", "skirt", "skull", "slate", "sleep",
    "slice", "slide", "sling", "sloop", "slump", "small", "smell", "smoke", "snack", "snake",
    "sneak", "sober", "solar", "solid", "solve", "sonar", "sonic", "sorry", "south", "space",
    "spare", "spark", "speak", "spear", "speed", "spell", "spend", "spent", "spill", "spine",
    "split", "spoke", "spoon", "sport", "spray", "squad", "stack", "staff", "stage", "stain",
    "stair", "stake", "stamp", "stand", "stark", "start", "state", "stash", "steak", "steal",
    "steam", "steel", "steep", "steer", "stern", "stick", "stiff", "still", "sting", "stock",
    "stomp", "stool", "store", "storm", "story", "stove", "strap", "straw", "stray", "strip",
    "stuck", "study", "stuff", "style", "suite", "sunny", "super", "surge", "swamp", "swarm",
    "swear", "sweat", "sweep", "sweet", "swell", "swift", "swing", "sword", "syrup", "tacky",
    "taken", "tally", "taper", "taste", "taunt", "teach", "tease", "tempo", "tense", "tenth",
    "thank", "theft", "their", "theme", "there", "these", "thick", "thief", "thigh", "thing",
    "think", "third", "thorn", "those", "three", "threw", "throw", "thumb", "tidal", "tight",
    "timer", "timid", "title", "today", "token", "tonic", "tooth", "topic", "torch", "total",
    "touch", "tough", "towel", "tower", "toxic", "trace", "track", "tract", "trade", "trail",
    "trait", "tramp", "tread", "treat", "trend", "trial", "tribe", "trick", "troop", "trout",
    "truck", "truly", "trunk", "trust", "truth", "tulip", "twice", "twist", "ultra", "uncle",
    "under", "undue", "unify", "union", "unite", "unity", "until", "upper", "upset", "urban",
    "usage", "usher", "usual", "utter", "vague", "valid", "valor", "value", "valve", "vapor",
    "vault", "venue", "verge", "verse", "video", "vigor", "villa", "vinyl", "virus", "visit",
    "vista", "vital", "vivid", "vocal", "voice", "voter", "vouch", "vowel", "wagon", "waist",
    "waste", "watch", "weary", "weave", "wedge", "weigh", "weird", "where", "which", "while",
    "whine", "whirl", "whole", "whose", "widen", "wider", "widow", "width", "wield", "windy",
    "witty", "woman", "woods", "worry", "worse", "worst", "worth", "would", "wound", "wring",
    "wrist", "write", "wrong", "yacht", "yearn", "yeast", "yield", "young", "youth", "zebra",
];
const GUESSES: &[&str] = &[
    "adore", "alert", "alien", "alone", "amber", "ample", "apple", "beach", "beard", "berry",
    "black", "blade", "bread", "brick", "bring", "brown", "chair", "chase", "chime", "clean",
    "clear", "climb", "clock", "cloud", "coral", "dance", "dream", "earth", "field", "flame",
    "fresh", "front", "giant", "glass", "grape", "green", "heart", "house", "ivory", "jolly",
    "kneel", "lemon", "light", "maple", "metal", "night", "ocean", "olive", "pearl", "plant",
    "proud", "river", "roast", "round", "shine", "shore", "smart", "smile", "sound", "spice",
    "stone", "sugar", "table", "tiger", "toast", "train", "water", "whale", "wheat", "world",
    "aback", "abate", "abbey", "abbot", "abhor", "abide", "abode", "abort", "abuzz", "acorn",
    "acrid", "adept", "adobe", "adorn", "afoot", "afoul", "agile", "aglow", "agony", "aided",
    "aired", "aisle", "algae", "alibi", "align", "allot", "aloft", "aloud", "amble", "amiss",
    "among", "amuse", "anvil", "aorta", "aphid", "aptly", "arbor", "ardor", "ascot", "ashen",
    "askew", "aspen", "augur", "aunty", "avert", "avian", "axial", "axiom", "azure", "baggy",
    "baize", "baler", "banal", "baron", "basal", "baste", "bathe", "batik", "bawdy", "beads",
    "beady", "beams", "beech", "beefy", "beeps", "befit", "began", "beget", "begin", "beige",
    "belch", "bells", "bends", "beret", "berms", "beset", "betas", "bezel", "bicep", "bigot",
    "bilge", "bills", "binge", "bingo", "birch", "birds", "bites", "blare", "blast", "blaze",
    "bleak", "bleat", "bleed", "bleep", "blimp", "bluer", "blues", "bluff", "blunt", "blurb",
    "blurt", "boils", "bolts", "bonds", "boned", "bonny", "borax", "bored", "bosom", "bossy",
    "botch", "bough", "bouts", "bowel", "bowls", "boxer", "brace", "brags", "braid", "brake",
    "brats", "brawn", "breve", "brews", "briar", "bribe", "brims", "broil", "brood", "brunt",
    "bucks", "budge", "buffs", "buggy", "bulbs", "bulge", "bulky", "bulls", "bully", "bumps",
    "bunks", "buoys", "burly", "burns", "burps", "burro", "bushy", "busts", "butte", "buxom",
    "bylaw", "bytes", "cabal", "cacti", "caddy", "cadet", "cadre", "cages", "cakes", "calms",
    "calve", "camps", "caned", "canes", "canny", "canon", "canto", "capes", "carat", "cards",
    "cared", "carol", "carps", "carts", "cased", "casks", "caste", "cater", "caves", "cease",
    "cello", "cents", "chafe", "chaff", "chaps", "chard", "chary", "chasm", "chats", "cheat",
    "chews", "chewy", "chide", "chili", "chill", "chimp", "chirp", "chive", "chock", "choke",
    "chops", "chuck", "chugs", "chump", "churn", "chute", "cider", "cigar", "cinch", "circa",
    "cited", "civet", "clack", "clads", "clams", "clang", "clank", "clans", "claps", "clash",
    "claws", "clays", "cleft", "clerk", "clews", "click", "cling", "clink", "clips", "clods",
    "clogs", "clone", "clots", "clout", "clove", "clubs", "cluck", "clues", "clump", "clung",
    "coals", "cobra", "cocoa", "coils", "coins", "cokes", "colds", "colic", "colts", "comas",
    "combo", "comer", "comfy", "comic", "comma", "conch", "condo", "cones", "conga", "cooks",
    "cooky", "cools", "coops", "copes", "copse", "cords", "cores", "corks", "corns", "corps",
    "costs", "couch", "cough", "coupe", "coves", "covet", "cowed", "cower", "cowls", "coyly",
    "crabs", "crags", "cramp", "crams", "crank", "craps", "crass", "craws", "craze", "crazy",
    "creak", "credo", "creed", "creek", "creep", "crepe", "crept", "crews", "cribs", "cried",
    "crimp", "croak", "crock", "crone", "crony", "crook", "croon", "crops", "crows", "crude",
    "crumb", "cruse", "crypt", "cubby", "cubed", "cubes", "cubic", "cubit", "cuffs", "culls",
    "cults", "cumin", "cupid", "curbs", "curds", "cured", "curie", "curls", "curly", "curse",
    "cusps", "cuter", "cutie", "czars", "dally", "dames", "damps", "dandy", "dared", "darts",
    "dated", "daunt", "dawns", "dazed", "deals", "deans", "dears", "death", "debar", "debit",
    "debts", "debug", "decks", "decoy", "decry", "deeds", "deems", "deeps", "defer", "deify",
    "deity", "dells", "delve", "demon", "demur", "denim", "dents", "depot", "derby", "desks",
    "deter", "detox", "deuce", "dials", "diced", "dicey", "diets", "dikes", "dills", "dimly",
    "dinar", "dined", "diner", "dingo", "dinky", "diode", "dirge", "disco", "disks", "ditto",
    "ditty", "divan", "divas", "dived", "diver", "divot", "docks", "dodos", "doers", "doggy",
    "dogma", "doily", "doled", "doles", "dolls", "dolly", "dolts", "domed", "domes", "donut",
    "dooms", "doors", "doped", "dopey", "dorky", "dorms", "dosed", "doted", "dotty", "douse",
    "doves", "dowdy", "dowel", "downs", "downy", "dowry", "dozed", "dozen", "drags", "drake",
    "drams", "draws", "drays", "dregs", "drier", "dries", "drips", "droll", "drool", "drops",
    "dross", "drown", "drugs", "drums", "dryad", "dryer", "duals", "duchy", "ducks", "ducts",
    "dudes", "duets", "dukes", "dulls", "dully", "dummy", "dumps", "dumpy", "dunce", "dunes",
    "dunks", "dusky", "dusts", "duvet", "dwarf", "dwelt", "dyers", "dying", "earls", "earns",
    "eased", "eases", "eater", "ebbed", "ebony", "echos", "edema", "edged", "edger", "edges",
    "edict", "edify", "edits", "eerie", "eight", "eject", "elate", "elegy", "elfin", "elide",
    "elope", "elude", "elves", "email", "embed", "ember", "emend", "emits", "emoji", "ended",
    "endow", "enema", "ennui", "ensue", "envoy", "epics", "epoch", "epoxy", "ergot", "erode",
    "erupt", "ester", "ether", "ethos", "etude", "evade", "evict", "evils", "evoke", "exalt",
    "exams", "excel", "exert", "exits", "expel", "extol", "exude", "exult", "eying", "faced",
    "facet", "facts", "fades", "fails", "fairs", "faked", "faker", "falls", "false", "famed",
    "fangs", "farce", "fared", "fares", "farms", "fasts", "fated", "fatty", "fauna", "fawns",
    "fears", "feats", "fecal", "feeds", "feels", "feign", "fells", "felon", "felts", "femur",
    "fends", "feral", "ferns", "fetal", "fetch", "fetid", "fetus", "feuds", "fewer", "filed",
    "filer", "files", "filet", "fills", "filly", "films", "filmy", "filth", "finch", "finds",
    "fined", "finer", "fired", "firms", "firth", "fishy", "fists", "fitly", "fiver", "fives",
    "fixed", "fixer", "fjord", "flags", "flail", "flake", "flaky", "flank", "flans", "flaps",
    "flare", "flash", "flats", "flaws", "fleas", "fleck", "flees", "flier", "flies", "fling",
    "flips", "flirt", "flogs", "floes", "flops", "flora", "floss", "flout", "flown", "flows",
    "flues", "fluff", "fluke", "flume", "flung", "flunk", "flute", "foals", "foams", "foamy",
    "foils", "foist", "folds", "folio", "folks", "folly", "fonts", "foods", "fools", "foots",
    "foray", "forks", "forms", "forte", "fount", "fowls", "foxes", "foyer", "frail", "frays",
    "freak", "freed", "freer", "friar", "fries", "frill", "frisk", "frizz", "frogs", "frond",
    "frown", "froze", "fudge", "fuels", "fumes", "funds", "fungi", "funky", "furls", "furor",
    "furry", "fused", "fuses", "fussy", "fuzzy", "gable", "gaffe", "gaily", "gains", "gaits",
    "galas", "gales", "galls", "games", "gamma", "gamut", "gangs", "gaped", "gases", "gasps",
    "gassy", "gated", "gates", "gaunt", "gauze", "gavel", "gawks", "gawky", "gears", "gecko",
    "geeks", "gents", "genus", "germs", "ghoul", "giddy", "gifts", "gilds", "girls", "girth",
    "gives", "gizmo", "glade", "glaze", "gleam", "glean", "glens", "glint", "gloat", "globe",
    "glows", "glued", "gnash", "gnats", "gnaws", "goads", "goals", "goats", "godly", "going",
    "goner", "gongs", "goods", "goody", "gooey", "goofy", "goose", "gored", "gorge", "gouge",
    "gourd", "grabs", "graft", "grate", "graze", "greed", "greys", "grime", "grimy", "gripe",
    "groin", "grope", "grubs", "gruel", "gruff", "grunt", "guild", "guile", "guise", "gulch",
    "gulfs", "gulls", "gully", "gulps", "gummy", "gusto", "gusts", "gutsy", "gypsy", "hacks",
    "haiku", "hails", "hairs", "hairy", "halls", "halos", "halts", "hands", "hangs", "harem",
    "harms", "harps", "hasty", "hawks", "heads", "heals", "heaps", "heard", "hears", "heath",
    "heats", "heels", "hefts", "heirs", "helix", "helps", "herbs", "herds", "hiked", "hiker",
    "hills", "hilly", "hinds", "hinge", "hints", "hippo", "hires", "hives", "hoard", "hobos",
    "hocks", "holds", "holed", "holes", "homed", "homes", "honed", "hooks", "hoops", "hoped",
    "horde", "horns", "hosed", "hosts", "hound", "hours", "howls", "hunch", "hunts", "hurts",
    "husks", "husky", "hutch", "hydra", "hyena", "icing", "icons", "idiom", "idled", "idler",
    "idols", "igloo", "ileum", "iliac", "imbue", "imped", "inane", "incur", "inept", "inert",
    "infer", "ingot", "inked", "inlay", "inlet", "inter", "irate", "irons", "irony", "isles",
    "itchy", "items", "jaded", "jails", "jaunt", "jeans", "jeers", "jerks", "jetty", "joins",
    "joked", "jolts", "jowls", "jumbo", "jumps", "jumpy", "junks", "junta", "juror", "keels",
    "keens", "kelps", "kerns", "kicks", "kills", "kilns", "kilts", "kinds", "kings", "kinks",
    "kinky", "kiwis", "knell", "knelt", "knobs", "knots", "koala", "laced", "laces", "lacks",
    "ladle", "lager", "lairs", "lakes", "lambs", "lames", "lamps", "lands", "lanes", "lanky",
    "lapel", "lapse", "larch", "lards", "larks", "latch", "lathe", "lauds", "lawns", "leaks",
    "leaky", "leaps", "leapt", "leash", "leech", "leeks", "leers", "lefty", "lemur", "lends",
    "lento", "leper", "liars", "licks", "liege", "liens", "lifts", "liked", "limbs", "limes",
    "limps", "lined", "lingo", "links", "lions", "lipid", "lists", "liter", "lithe", "lived",
    "liven", "loads", "loafs", "loams", "loans", "loath", "lobes", "locks", "locus", "lodes",
    "lofts", "loins", "lolls", "loner", "longs", "looks", "looms", "loops", "loots", "lords",
    "loses", "lotto", "lotus", "louse", "louts", "loved", "lowly", "lucid", "lulls", "lumps",
    "lumpy", "lunar", "lunge", "lungs", "lurch", "lured", "lurks", "lusts", "lusty", "lying",
    "lymph", "macho", "macro", "madam", "maids", "mails", "maims", "mains", "maize", "males",
    "malls", "malts", "mamas", "manes", "mango", "manic", "mares", "marks", "marsh", "masks",
    "mason", "masts", "mates", "matte", "mauve", "maxed", "maxim", "maybe", "meals", "mealy",
    "means", "meats", "meaty", "mecca", "medic", "meets", "melts", "memos", "mends", "menus",
    "meted", "metro", "mewed", "micro", "miens", "milds", "miles", "milks", "milky", "mince",
    "minds", "mined", "miner", "mines", "mints", "minty", "mired", "miser", "misty", "miter",
    "mixed", "mixer", "moans", "moats", "mocks", "modal", "modes", "mogul", "molar", "molds",
    "molls", "molts", "momma", "monks", "monos", "moods", "moody", "moons", "moose", "moped",
    "moray", "mores", "mossy", "motif", "mousy", "moved", "mover", "moves", "mowed", "mower",
    "mucus", "muffs", "muggy", "mulch", "mules", "mummy", "mumps", "murky", "mused", "muses",
    "mushy", "musky", "musts", "musty", "muted", "mutts", "mylar", "myths", "nadir", "nails",
    "naked", "named", "names", "nanny", "nasal", "natal", "navel", "nears", "necks", "needs",
    "needy", "neigh", "nests", "newsy", "nexus", "niche", "nicks", "niece", "nines", "ninth",
    "nodal", "nodes", "nomad", "nonce", "noose", "norms", "nosed", "noses", "notch", "nouns",
    "nudge", "nudes", "nukes", "nulls", "nutty", "oaken", "oakum", "oaths", "obese", "obeys",
    "octal", "octet", "odder", "oddly", "odium", "odors", "ogled", "ogres", "oiled", "olden",
    "older", "omega", "omens", "omits", "onset", "opals", "opens", "opium", "opted", "optic",
    "otter", "ounce", "outdo", "outgo", "ovals", "ovary", "ovens", "overt", "paced", "pacer",
    "packs", "pacts", "paddy", "pagan", "paged", "pager", "pages", "pails", "pains", "pairs",
    "palms", "palsy", "panda", "paned", "panes", "pangs", "pansy", "pants", "papal", "parch",
    "pared", "parks", "parse", "parts", "passe", "pasts", "pasty", "paths", "paved", "pawed",
    "pawns", "payee", "payer", "peaks", "peats", "pecan", "peeks", "peels", "peeps", "peers",
    "pelts", "penal", "pence", "penne", "peony", "perks", "pesky", "pesos", "pests", "petal",
    "petty", "picks", "picky", "piety", "piggy", "pikes", "piled", "piles", "pills", "pined",
    "pines", "pints", "pinup", "pious", "piped", "piper", "pipes", "pique", "pithy", "plaid",
    "plank", "plans", "pleas", "plied", "plods", "plops", "plots", "ploys", "pluck", "plugs",
    "plumb", "plume", "plump", "plums", "plush", "poach", "pocks", "poems", "poets", "poise",
    "poked", "poker", "pokes", "poled", "poles", "polls", "polyp", "ponds", "pooch", "pools",
    "poppy", "pored", "pores", "ports", "posed", "poser", "poses", "posit", "posse", "posts",
    "pouch", "pours", "pouts", "prank", "prate", "prawn", "preen", "pried", "prism", "privy",
    "prowl", "prude", "prune", "psalm", "pubic", "pudgy", "puffs", "pulpy", "pumps", "puppy",
    "pushy", "putts", "quack", "quail", "qualm", "quark", "quart", "quash", "quasi", "queer",
    "quell", "quill", "quirk", "quite", "rabbi", "rabid", "raced", "racer", "races", "racks",
    "rafts", "raged", "rains", "rajah", "ramps", "ranks", "rarer", "raspy", "rated", "rates",
    "ratty", "raved", "raven", "rawly", "rayon", "reads", "reams", "reaps", "rears", "rebus",
    "recap", "reeds", "reefs", "reels", "reins", "relic", "remit", "renal", "repel", "rerun",
    "rests", "retch", "retro", "retry", "reuse", "rhino", "rider", "rides", "rifle", "right",
    "rigor", "riots", "ripen", "risen", "riser", "risks", "risky", "rites", "rivet", "roams",
    "robed", "robes", "rocks", "rodeo", "roles", "rolls", "roofs", "rooks", "rooms", "roomy",
    "roots", "ropes", "roses", "rouge", "rouse", "roved", "rover", "rowdy", "ruble", "ruddy",
    "ruder", "ruins", "ruled", "rules", "rumba", "runes", "rungs", "runny", "rupee", "rushy",
    "saber", "sable", "sadly", "safer", "sales", "salty", "salve", "saner", "sappy", "satin",
    "sauna", "saved", "savor", "scalp", "scamp", "scans", "scant", "scoff", "scold", "scone",
    "scoop", "scorn", "scour", "scowl", "scram", "scree", "scrub", "scuba", "scuff", "seals",
    "seams", "sears", "seats", "seeds", "seeks", "seems", "seeps", "seine", "seize", "sells",
    "sends", "sepia", "serge", "serum", "setup", "sever", "sewed", "sewer", "shack", "shady",
    "shaft", "shard", "shark", "shawl", "shear", "sheds", "shims", "shiny", "ships", "shoal",
    "shoed", "shoes", "shone", "shook", "shops", "shots", "shove", "shows", "shred", "shrew",
    "shrub", "shrug", "shuck", "shuns", "shunt", "shush", "shuts", "shyly", "sided", "sides",
    "siege", "sieve", "sighs", "sigma", "signs", "silks", "silky", "silos", "silts", "singe",
    "sired", "siren", "sites", "sixes", "sized", "sizes", "skeet", "skied", "skier", "skies",
    "skiff", "skimp", "skins", "skint", "skits", "skunk", "slabs", "slack", "slain", "slams",
    "slang", "slant", "slaps", "slash", "slave", "sleds", "sleek", "sleet", "slept", "slick",
    "slime", "slimy", "slink", "slips", "sloes", "slosh", "sloth", "slots", "slows", "slugs",
    "slums", "slung", "slurp", "slush", "smack", "smash", "smear", "smelt", "smirk", "smite",
    "smith", "smock", "smoky", "snail", "snaps", "snare", "snarl", "sneer", "snide", "sniff",
    "snipe", "snore", "snout", "snowy", "snubs", "snuff", "soaks", "soaps", "soapy", "soars",
    "socks", "soggy", "soils", "soled", "soles", "songs", "sooth", "sooty", "sores", "sorts",
    "souls", "soups", "sours", "sowed", "spade", "spars", "spasm", "spawn", "speck", "sperm",
    "spicy", "spied", "spiel", "spike", "spins", "spiny", "spire", "spite", "spoil", "spoof",
    "spook", "spool", "spore", "spots", "spout", "sprat", "spree", "sprig", "spunk", "spurn",
    "spurs", "squat", "stale", "stalk", "stars", "stave", "stays", "stead", "steed", "stems",
    "stent", "steps", "stile", "stink", "stint", "stoic", "stony", "stood", "stoop", "stops",
    "stout", "strep", "strew", "strut", "stubs", "stump", "stung", "stunk", "stunt", "suave",
    "sucks", "suede", "sulks", "sulky", "sumac", "surly", "swabs", "swain", "swank", "swaps",
    "swath", "swept", "swine", "swipe", "swirl", "swish", "swoop", "swore", "sworn", "tabby",
    "tails", "taint", "takes", "tales", "talks", "tamed", "tamer", "tango", "tangy", "tanks",
    "taped", "tapes", "tardy", "tarot", "tarps", "tasks", "tasty", "taupe", "tawny", "taxed",
    "taxis", "teaks", "teals", "teams", "tears", "teary", "teddy", "teens", "teeth", "tells",
    "tempt", "tends", "tenet", "tenor", "tents", "terms", "terse", "tests", "testy", "texts",
    "thaws", "therm", "thump", "tiara", "tides", "tiers", "tiles", "tines", "tinge", "tipsy",
    "tired", "tires", "tithe", "tolls", "tombs", "tomes", "tonal", "toned", "toner", "tones",
    "topaz", "torso", "torte", "tours", "towns", "trams", "traps", "trash", "treed", "treks",
    "tried", "tries", "trill", "trips", "trite", "troll", "trove", "truce", "tubas", "tuber",
    "tubes", "tucks", "tulle", "tumor", "tuned", "tuner", "tunes", "turbo", "turns", "tutor",
    "tweak", "tweed", "tweet", "twigs", "twins", "tying", "typed", "types", "udder", "ulcer",
    "umbra", "unwed", "urges", "vales", "valet", "vases", "veers", "veins", "venom", "vents",
    "verbs", "verve", "vests", "vicar", "views", "vigil", "vines", "viola", "viper", "viral",
    "vodka", "vogue", "volts", "voted", "votes", "vowed", "waded", "wader", "wades", "wafer",
    "wafts", "waged", "wages", "waits", "waive", "wakes", "walks", "walls", "waltz", "wants",
    "wards", "warns", "warts", "wasps", "waved", "waver", "waves", "waxed", "weeds", "weeks",
    "weeps", "wells", "wench", "wharf", "wheel", "whisk", "white", "wicks", "wiles", "wills",
    "wimps", "wined", "wines", "wings", "winks", "wiped", "wiper", "wires", "wiser", "wispy",
    "witch", "wives", "woken", "wombs", "woody", "wooly", "words", "works", "worms", "wraps",
    "wrath", "wreck", "wrens", "writs", "wrote", "yarns", "yodel", "yogis", "yokes", "yours",
    "zeros", "zesty", "zonal", "zones",
];
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Mark {
    Absent,
    Present,
    Placed,
}
fn djb2(s: &str) -> u64 {
    s.bytes().fold(5381_u64, |h, b| {
        h.wrapping_mul(33).wrapping_add(u64::from(b))
    })
}
fn answer_for(date: &str) -> &'static str {
    let answer_count = u64::try_from(ANSWERS.len()).expect("the answer list fits u64");
    let index = usize::try_from(djb2(&format!("{date}{SALT}")) % answer_count)
        .expect("the reduced answer index fits usize");
    ANSWERS[index]
}
fn marks(answer: &str, guess: &str) -> [Mark; 5] {
    let mut out = [Mark::Absent; 5];
    let a = answer.as_bytes();
    let g = guess.as_bytes();
    let mut used = [false; 5];
    for i in 0..5 {
        if g[i] == a[i] {
            out[i] = Mark::Placed;
            used[i] = true;
        }
    }
    for i in 0..5 {
        if out[i] != Mark::Placed {
            if let Some(j) = (0..5).find(|&j| !used[j] && a[j] == g[i]) {
                out[i] = Mark::Present;
                used[j] = true;
            }
        }
    }
    out
}
fn valid(word: &str) -> bool {
    ANSWERS.contains(&word) || GUESSES.contains(&word)
}
const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
fn civil_date(days_since_epoch: i64) -> String {
    let shifted = days_since_epoch + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}
fn parse_date(date: &str) -> Option<(i64, i64, i64)> {
    let mut parts = date.split('-');
    let year = parts.next()?.parse().ok()?;
    let month = parts.next()?.parse().ok()?;
    let day = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some((year, month, day))
}
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_prime = (month + 9) % 12;
    let day_of_year = (153 * month_prime + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}
fn shift_date(date: &str, delta: i64) -> String {
    let Some((year, month, day)) = parse_date(date) else {
        return date.to_owned();
    };
    civil_date(days_from_civil(year, month, day) + delta)
}
fn full_date(date: &str) -> String {
    let Some((year, month, day)) = parse_date(date) else {
        return date.to_owned();
    };
    format!(
        "{} {}, {}",
        MONTHS[usize::try_from(month - 1).unwrap_or(0)],
        day,
        year
    )
}
fn short_date(date: &str) -> String {
    let Some((_, month, day)) = parse_date(date) else {
        return date.to_owned();
    };
    format!(
        "{} {}",
        &MONTHS[usize::try_from(month - 1).unwrap_or(0)][..3],
        day
    )
}
fn today() -> String {
    let days = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() / 86_400);
    civil_date(i64::try_from(days).unwrap_or(i64::MAX))
}
fn hard_allows(answer: &str, prior: &[String], guess: &str) -> bool {
    let guess_bytes = guess.as_bytes();
    let mut required = [0_u8; 26];
    for old in prior {
        let old_bytes = old.as_bytes();
        let scored = marks(answer, old);
        let mut seen = [0_u8; 26];
        for (index, mark) in scored.into_iter().enumerate() {
            let letter = old_bytes[index];
            if mark == Mark::Placed && guess_bytes[index] != letter {
                return false;
            }
            if mark == Mark::Present && guess_bytes[index] == letter {
                return false;
            }
            if mark != Mark::Absent {
                let offset = usize::from(letter.saturating_sub(b'a'));
                if offset < seen.len() {
                    seen[offset] = seen[offset].saturating_add(1);
                }
            }
        }
        for (need, count) in required.iter_mut().zip(seen) {
            *need = (*need).max(count);
        }
    }
    required.iter().enumerate().all(|(offset, needed)| {
        let letter = b'a' + u8::try_from(offset).expect("alphabet index");
        guess_bytes.iter().fold(0_usize, |count, candidate| {
            count + usize::from(*candidate == letter)
        }) >= usize::from(*needed)
    })
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LoadState {
    Pending,
    Ready,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum View {
    Board,
    Help,
    Stats,
    Archive,
}

struct Game {
    today: String,
    date: String,
    archive: bool,
    archive_page: usize,
    daily: Vec<String>,
    archive_guesses: Vec<String>,
    keyboard: Keyboard,
    notice: String,
    export_note: Option<String>,
    hard: bool,
    typing: bool,
    view: View,
    load: LoadState,
    played: u32,
    wins: u32,
    dist: [u32; 6],
}
impl Default for Game {
    fn default() -> Self {
        let date = std::env::var("KOBO_INKLING_DAY").unwrap_or_else(|_| today());
        Self::for_day(&date)
    }
}
impl Game {
    fn for_day(date: &str) -> Self {
        Self {
            today: date.to_owned(),
            date: date.to_owned(),
            archive: false,
            archive_page: 0,
            daily: Vec::new(),
            archive_guesses: Vec::new(),
            keyboard: Keyboard::new(),
            notice: "Six guesses. Shapes, not color, carry the state.".into(),
            export_note: None,
            hard: false,
            typing: false,
            view: View::Board,
            load: LoadState::Pending,
            played: 0,
            wins: 0,
            dist: [0; 6],
        }
    }
    fn guesses(&self) -> &Vec<String> {
        if self.archive {
            &self.archive_guesses
        } else {
            &self.daily
        }
    }
    fn answer(&self) -> &'static str {
        answer_for(&self.date)
    }
}
impl Game {
    fn encode(&self) -> Vec<u8> {
        format!(
            "2|{}|{}|{}|{}|{}|{}",
            self.today,
            u8::from(self.hard),
            self.played,
            self.wins,
            self.dist
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(","),
            self.daily.join(",")
        )
        .into_bytes()
    }

    fn restore(&mut self, bytes: &[u8]) -> bool {
        let Ok(text) = std::str::from_utf8(bytes) else {
            return false;
        };
        let fields = text.split('|').collect::<Vec<_>>();
        let version = match fields.first().copied() {
            Some("1") => 1,
            Some("2") => 2,
            _ => return false,
        };
        if fields.len() != version + 5 {
            return false;
        }
        let hard = match fields.get(2).copied() {
            Some("0") => false,
            Some("1") => true,
            _ => return false,
        };
        let (Ok(played), Ok(wins)) = (fields[3].parse::<u32>(), fields[4].parse::<u32>()) else {
            return false;
        };
        if wins > played {
            return false;
        }
        let mut dist = [0_u32; 6];
        if version == 2 {
            let counts = fields[5].split(',').collect::<Vec<_>>();
            if counts.len() != 6 {
                return false;
            }
            for (slot, count) in dist.iter_mut().zip(counts) {
                let Ok(parsed) = count.parse::<u32>() else {
                    return false;
                };
                *slot = parsed;
            }
            if dist.iter().sum::<u32>() > wins {
                return false;
            }
        }
        let guesses_field = fields[version + 4];
        let guesses = if guesses_field.is_empty() {
            Vec::new()
        } else {
            guesses_field
                .split(',')
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        if guesses.len() > 6 || guesses.iter().any(|guess| !valid(guess)) {
            return false;
        }
        let answer = answer_for(fields[1]);
        if guesses
            .iter()
            .take(guesses.len().saturating_sub(1))
            .any(|guess| guess == answer)
        {
            return false;
        }
        let completed = guesses.len() == 6 || guesses.last().is_some_and(|guess| guess == answer);
        if completed
            && (played == 0 || (guesses.last().is_some_and(|guess| guess == answer) && wins == 0))
        {
            return false;
        }
        // Validate completely before replacing anything; a bad save cannot
        // leave a partially restored board or index a short guess in marks().
        self.played = played;
        self.wins = wins;
        self.dist = dist;
        self.hard = hard;
        if fields[1] == self.today {
            self.daily = guesses;
            self.notice = if self.daily.last().is_some_and(|guess| guess == answer) {
                "Solved.".into()
            } else if self.done() {
                format!("Answer: {}", self.answer().to_ascii_uppercase())
            } else {
                "Saved game restored.".into()
            };
        }
        true
    }

    fn done(&self) -> bool {
        let answer = self.answer();
        self.guesses().len() >= 6 || self.guesses().last().is_some_and(|guess| *guess == answer)
    }

    fn submit(&mut self) {
        if self.done() {
            return;
        }
        let guess = self.keyboard.take().to_ascii_lowercase();
        if guess.len() != 5 {
            self.notice = "Use five letters.".into();
            return;
        }
        if !valid(&guess) {
            self.notice = "Not in the word list.".into();
            return;
        }
        if self.hard && !hard_allows(self.answer(), self.guesses(), &guess) {
            self.notice = "Hard mode requires every revealed letter and position.".into();
            return;
        }
        let answer = self.answer();
        if self.archive {
            self.archive_guesses.push(guess.clone());
        } else {
            self.daily.push(guess.clone());
        }
        if !self.archive && self.done() {
            self.played = self.played.saturating_add(1);
            if guess == answer {
                self.wins = self.wins.saturating_add(1);
                let slot = self.daily.len().saturating_sub(1).min(5);
                self.dist[slot] = self.dist[slot].saturating_add(1);
            }
        }
        self.notice = if guess == answer {
            if self.archive {
                "Solved. Archive games do not change statistics.".into()
            } else {
                "Solved.".into()
            }
        } else if self.done() {
            format!("Answer: {}", answer.to_ascii_uppercase())
        } else {
            format!("{} of 6", self.guesses().len())
        };
    }

    fn cell(&self, row: usize, col: usize) -> String {
        if let Some(guess) = self.guesses().get(row) {
            let letter = guess.chars().nth(col).unwrap_or(' ').to_ascii_uppercase();
            match marks(self.answer(), guess)[col] {
                Mark::Placed => format!("[{letter}]"),
                Mark::Present => format!("({letter})"),
                Mark::Absent => format!("{letter}\u{00d7}"),
            }
        } else {
            " ".into()
        }
    }

    fn knowledge(&self) -> String {
        if self.guesses().is_empty() {
            return "No letters known yet.".into();
        }
        let answer = self.answer();
        let mut slots = ['_'; 5];
        let mut present = Vec::new();
        let mut absent = Vec::new();
        for letter in b'a'..=b'z' {
            let mut best = Mark::Absent;
            let mut seen = false;
            for guess in self.guesses() {
                for (index, mark) in marks(answer, guess).into_iter().enumerate() {
                    if guess.as_bytes()[index] == letter {
                        seen = true;
                        best = match (best, mark) {
                            (Mark::Placed, _) | (_, Mark::Placed) => Mark::Placed,
                            (Mark::Present, _) | (_, Mark::Present) => Mark::Present,
                            _ => Mark::Absent,
                        };
                        if mark == Mark::Placed {
                            slots[index] = (letter as char).to_ascii_uppercase();
                        }
                    }
                }
            }
            if !seen {
                continue;
            }
            let upper = (letter as char).to_ascii_uppercase();
            match best {
                Mark::Placed => {}
                Mark::Present => present.push(upper.to_string()),
                Mark::Absent => absent.push(upper.to_string()),
            }
        }
        let mut parts = Vec::new();
        if slots.iter().any(|slot| *slot != '_') {
            parts.push(format!("Placed: {}", slots.iter().collect::<String>()));
        }
        if !present.is_empty() {
            parts.push(format!("In word: {}", present.join(" ")));
        }
        if !absent.is_empty() {
            parts.push(format!("Out: {}", absent.join(" ")));
        }
        parts.join("  \u{00b7}  ")
    }

    fn export_text(&self) -> String {
        let mut out = format!("Inkling, {}\n", full_date(&self.today));
        let answer = answer_for(&self.today);
        if self.daily.is_empty() {
            out.push_str("Today's puzzle is not started.\n");
        } else {
            if self.daily.last().is_some_and(|guess| *guess == answer) {
                out.push_str(&format!("Solved in {} of 6.\n", self.daily.len()));
            } else if self.daily.len() >= 6 {
                out.push_str(&format!(
                    "Not solved. The answer was {}.\n",
                    answer.to_ascii_uppercase()
                ));
            } else {
                out.push_str(&format!(
                    "In progress: {} of 6 guesses.\n",
                    self.daily.len()
                ));
            }
            for guess in &self.daily {
                let cells = (0..5)
                    .map(|col| {
                        let letter = guess.chars().nth(col).unwrap_or(' ').to_ascii_uppercase();
                        match marks(answer, guess)[col] {
                            Mark::Placed => format!("[{letter}]"),
                            Mark::Present => format!("({letter})"),
                            Mark::Absent => format!("{letter}\u{00d7}"),
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                out.push_str(&cells);
                out.push('\n');
            }
        }
        out.push_str(&format!("\nPlayed {}. Won {}.\n", self.played, self.wins));
        for (index, count) in self.dist.iter().enumerate() {
            if *count > 0 {
                out.push_str(&format!("Solved in {}: {}\n", index + 1, count));
            }
        }
        out
    }

    fn board(&self) -> Screen {
        let title = if self.archive {
            format!("Archive \u{00b7} {}", short_date(&self.date))
        } else {
            format!("Inkling \u{00b7} {}", short_date(&self.today))
        };
        let cells = (0..30).map(|i| (format!("cell-{i}"), self.cell(i / 5, i % 5)));
        let builder = ScreenBuilder::new("inkling")
            .top_bar(title)
            .top_bar_glyph("how-to-play", "How to play", Glyph::Note)
            .secondary(&self.notice)
            .grid(5, false, cells);
        let builder = if self.done() {
            builder
        } else {
            builder.button("enter", "Enter guess")
        };
        builder
            .action_bar([
                (
                    "hard",
                    if self.hard {
                        "Hard mode on"
                    } else {
                        "Hard mode off"
                    },
                ),
                ("stats", "Stats"),
                if self.archive {
                    ("today", "Today")
                } else {
                    ("archive", "Archive")
                },
            ])
            .build()
    }

    fn help_screen() -> Screen {
        ScreenBuilder::new("inkling-help")
            .top_bar("How to play")
            .owns_back(true)
            .heading("Find the five-letter word")
            .text("You have six guesses. Type five letters, then tap Guess.")
            .text("[A] is in the right spot. (A) is elsewhere in the word. A\u{00d7} is absent.")
            .text("Known letters show while you type. Hard mode reuses placed letters.")
            .bottom_action("close-help", "Play")
            .build()
    }

    fn stats_screen(&self) -> Screen {
        let mut builder = ScreenBuilder::new("inkling-stats")
            .top_bar("Statistics")
            .owns_back(true)
            .heading("Daily puzzles")
            .text(format!("Played {}. Won {}.", self.played, self.wins));
        if self.played > 0 && self.dist.iter().all(|count| *count == 0) {
            builder = builder.text("Guess counts are recorded from version 0.2.0 on.");
        }
        for (index, count) in self.dist.iter().enumerate() {
            if *count > 0 {
                builder = builder.text(format!("Solved in {} of 6: {}", index + 1, count));
            }
        }
        builder
            .text(format!("Today is {}.", full_date(&self.today)))
            .button("export", "Export results")
            .bottom_action("close-stats", "Play")
            .build()
    }

    fn archive_screen(&self) -> Screen {
        let start = self.archive_page * ARCHIVE_PAGE;
        let mut rows = Vec::new();
        for i in 0..ARCHIVE_PAGE {
            let back = i64::try_from(start + i + 1).unwrap_or(i64::MAX);
            if back > ARCHIVE_DAYS {
                break;
            }
            let date = shift_date(&self.today, -back);
            rows.push((
                format!("day-{i}"),
                full_date(&date),
                String::new(),
                Glyph::Clock,
            ));
        }
        if i64::try_from(start + ARCHIVE_PAGE).unwrap_or(i64::MAX) < ARCHIVE_DAYS {
            rows.push((
                "earlier".to_owned(),
                "Earlier puzzles".to_owned(),
                String::new(),
                Glyph::More,
            ));
        }
        ScreenBuilder::new("inkling-archive")
            .top_bar("Archive")
            .owns_back(true)
            .text("Play a past day. Archive games do not change statistics and are not saved.")
            .rows(rows)
            .bottom_action("close-archive", "Back")
            .build()
    }

    fn typing_screen(&self) -> Screen {
        ScreenBuilder::new("inkling")
            .top_bar(if self.archive { "Archive" } else { "Inkling" })
            .typed(&self.keyboard, "Type five letters")
            .text(self.knowledge())
            .keyboard(&self.keyboard, "Guess")
            .bottom_action("cancel", "Cancel")
            .build()
    }

    fn open_archive_day(&mut self, index: usize) {
        let back = i64::try_from(self.archive_page * ARCHIVE_PAGE + index + 1).unwrap_or(1);
        self.date = shift_date(&self.today, -back);
        self.archive = true;
        self.archive_guesses.clear();
        self.notice = format!(
            "Archive puzzle from {}. Statistics count today's game only.",
            full_date(&self.date)
        );
        self.view = View::Board;
    }

    fn screen(&self) -> Screen {
        match self.view {
            View::Help => Self::help_screen(),
            View::Stats => self.stats_screen(),
            View::Archive => self.archive_screen(),
            View::Board => {
                if self.typing {
                    self.typing_screen()
                } else {
                    self.board()
                }
            }
        }
    }
}
impl KoboApp for Game {
    fn on_start(&mut self, c: &mut Context) {
        c.store().load(STATE);
        c.set_screen(self.screen());
    }
    fn on_store(&mut self, c: &mut Context, result: StoreResult) {
        match result {
            StoreResult::Loaded { key, value }
                if key == STATE && self.load == LoadState::Pending =>
            {
                if value.is_some_and(|bytes| !self.restore(&bytes)) {
                    self.notice = "Saved game was damaged and was ignored.".into();
                }
                self.load = LoadState::Ready;
            }
            StoreResult::Saved { key } if key == EXPORT => {
                self.export_note = Some(format!("Results written to {EXPORT}."));
            }
            StoreResult::Denied(_) => {
                self.load = LoadState::Ready;
                self.notice = "Progress could not be saved. Check available storage.".into();
            }
            _ => return,
        }
        c.set_screen(self.screen());
    }
    fn on_action(&mut self, c: &mut Context, a: ActionId) {
        if self.load == LoadState::Pending {
            return;
        }
        let mut changed = false;
        let mut save = false;
        match self.view {
            View::Help => {
                if a == action_id("close-help") || a == ActionId::BACK {
                    self.view = View::Board;
                    changed = true;
                }
            }
            View::Stats => {
                if a == action_id("close-stats") || a == ActionId::BACK {
                    self.export_note = None;
                    self.view = View::Board;
                    changed = true;
                } else if a == action_id("export") {
                    let text = self.export_text();
                    c.store().save(EXPORT, text.into_bytes());
                    self.export_note = Some("Writing results.".into());
                    changed = true;
                }
            }
            View::Archive => {
                if a == action_id("close-archive") || a == ActionId::BACK {
                    self.view = View::Board;
                    changed = true;
                } else if a == action_id("earlier") {
                    self.archive_page += 1;
                    changed = true;
                } else if let Some(index) =
                    (0..ARCHIVE_PAGE).find(|index| a == action_id(&format!("day-{index}")))
                {
                    self.open_archive_day(index);
                    changed = true;
                }
            }
            View::Board => {
                if self.typing {
                    if let Some(p) = self.keyboard.press(a) {
                        changed = true;
                        if p == Pressed::Submitted && !self.done() {
                            self.submit();
                            save = !self.archive;
                            self.typing = false;
                        }
                    } else if a == action_id("cancel") {
                        self.typing = false;
                        changed = true;
                    }
                } else if a == action_id("enter") && !self.done() {
                    self.typing = true;
                    changed = true;
                } else if a == action_id("hard") {
                    save = true;
                    self.hard = !self.hard;
                    self.notice = if self.hard {
                        "Hard mode on."
                    } else {
                        "Hard mode off."
                    }
                    .into();
                    changed = true;
                } else if a == action_id("stats") {
                    self.view = View::Stats;
                    changed = true;
                } else if a == action_id("archive") {
                    self.archive_page = 0;
                    self.view = View::Archive;
                    changed = true;
                } else if a == action_id("today") && self.archive {
                    self.archive = false;
                    self.date = self.today.clone();
                    let answer = self.answer();
                    self.notice = if self.daily.last().is_some_and(|guess| *guess == answer) {
                        "Solved.".into()
                    } else if self.done() {
                        format!("Answer: {}", answer.to_ascii_uppercase())
                    } else {
                        "Back to today's puzzle.".into()
                    };
                    changed = true;
                } else if a == action_id("how-to-play") {
                    self.view = View::Help;
                    changed = true;
                }
            }
        }
        if save {
            c.store().save(STATE, self.encode());
        }
        if changed {
            c.set_screen(self.screen());
        }
    }
}
fn main() -> ExitCode {
    match kobo_sdk::run("inkling", Game::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("inkling: {e}");
            ExitCode::FAILURE
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use kobo_ui::{Chrome, CLARA_BW_METRICS};
    #[test]
    fn dates_are_deterministic() {
        for y in 2020..2031 {
            let d = format!("{y}-09-01");
            assert_eq!(answer_for(&d), answer_for(&d));
        }
    }
    #[test]
    fn unix_days_format_as_real_calendar_dates() {
        assert_eq!(civil_date(0), "1970-01-01");
        assert_eq!(civil_date(20_697), "2026-09-01");
    }
    #[test]
    fn civil_and_days_round_trip() {
        for day in (-200_000_i64..200_000).step_by(397) {
            let date = civil_date(day);
            let (year, month, day_of_month) = parse_date(&date).expect("civil date parses");
            assert_eq!(days_from_civil(year, month, day_of_month), day, "{date}");
        }
    }
    #[test]
    fn dates_display_as_real_dates() {
        assert_eq!(full_date("2026-09-01"), "September 1, 2026");
        assert_eq!(short_date("2026-09-01"), "Sep 1");
        assert_eq!(shift_date("2026-09-01", -1), "2026-08-31");
        assert_eq!(shift_date("2026-01-01", -1), "2025-12-31");
        assert_eq!(shift_date("2026-03-01", -1), "2026-02-28");
    }
    #[test]
    fn word_lists_are_audited() {
        for word in ANSWERS.iter().chain(GUESSES) {
            assert!(word.len() == 5, "{word}");
            assert!(word.bytes().all(|b| b.is_ascii_lowercase()), "{word}");
        }
        for (index, word) in ANSWERS.iter().enumerate() {
            assert!(
                !ANSWERS[index + 1..].contains(word),
                "duplicate answer {word}"
            );
            assert!(!GUESSES.contains(word), "answer {word} repeated as a guess");
        }
        for (index, word) in GUESSES.iter().enumerate() {
            assert!(
                !GUESSES[index + 1..].contains(word),
                "duplicate guess {word}"
            );
        }
        // Anchors from the original seed list stay valid across the expansion.
        for anchor in ["crane", "caper", "stare", "adore", "toast", "world"] {
            assert!(valid(anchor), "{anchor}");
        }
        assert!(ANSWERS.len() >= 365, "a year of answers");
    }
    #[test]
    fn duplicate_letters_are_scored_once() {
        assert_eq!(
            marks("bloom", "ooooo"),
            [
                Mark::Absent,
                Mark::Absent,
                Mark::Placed,
                Mark::Placed,
                Mark::Absent
            ]
        );
    }
    #[test]
    fn hard_mode_reuses_present_letters_and_fixed_positions() {
        let prior = vec!["crane".to_owned()];
        assert!(hard_allows("caper", &prior, "cater"));
        assert!(!hard_allows("caper", &prior, "slope"));
        assert!(!hard_allows("caper", &prior, "crown"));
    }
    #[test]
    fn knowledge_lists_known_letters() {
        // The pinned day's answer is deterministic, so the line is exact.
        let mut game = Game::for_day("2026-09-01");
        game.archive = true;
        game.archive_guesses = vec!["crane".into()];
        assert_eq!(game.knowledge(), "Placed: _RA__  \u{00b7}  Out: C E N");
        game.archive_guesses.push("gravy".into());
        assert_eq!(game.knowledge(), "Placed: GRAVY  \u{00b7}  Out: C E N");
    }
    #[test]
    fn knowledge_is_empty_before_guesses() {
        let game = Game::for_day("2026-09-01");
        assert_eq!(game.knowledge(), "No letters known yet.");
    }
    #[test]
    fn statistics_count_daily_games_with_distribution() {
        let mut game = Game::for_day("2026-09-01");
        let answer = game.answer().to_owned();
        let filler = if answer == "grape" { "adore" } else { "grape" };
        game.keyboard = Keyboard::with_text(filler);
        game.submit();
        game.keyboard = Keyboard::with_text(answer);
        game.submit();
        assert_eq!((game.played, game.wins), (1, 1));
        assert_eq!(game.dist, [0, 1, 0, 0, 0, 0]);
    }
    #[test]
    fn losses_count_as_played_without_a_distribution_slot() {
        let mut game = Game::for_day("2026-09-01");
        let answer = game.answer();
        let filler = ["grape", "adore", "stare", "crane", "bloom", "flint"]
            .into_iter()
            .find(|word| *word != answer)
            .expect("a filler word");
        for _ in 0..6 {
            game.keyboard = Keyboard::with_text(filler);
            game.submit();
        }
        assert!(game.done());
        assert_eq!((game.played, game.wins), (1, 0));
        assert_eq!(game.dist, [0; 6]);
    }
    #[test]
    fn archive_games_never_touch_statistics() {
        let mut game = Game::for_day("2026-09-01");
        game.date = shift_date(&game.today, -1);
        game.archive = true;
        let answer = game.answer().to_owned();
        game.keyboard = Keyboard::with_text(answer);
        game.submit();
        assert!(game.done());
        assert_eq!((game.played, game.wins), (0, 0));
        assert_eq!(game.dist, [0; 6]);
        assert!(game.notice.contains("do not change statistics"));
    }
    #[test]
    fn version_one_saves_migrate_without_distribution() {
        let mut game = Game::for_day("2026-09-01");
        assert!(game.restore(b"1|2026-09-01|0|4|3|"));
        assert_eq!((game.played, game.wins), (4, 3));
        assert_eq!(game.dist, [0; 6]);
        let mut finished = Game::for_day("2026-09-01");
        let answer = answer_for("2026-09-01");
        let save = format!("1|2026-09-01|0|4|3|adore,{answer}");
        assert!(finished.restore(save.as_bytes()));
        assert_eq!(finished.daily.len(), 2);
        assert!(finished.done());
    }
    #[test]
    fn corrupt_guesses_are_rejected_before_they_reach_scoring() {
        for bytes in [
            b"2|2026-09-01|0|1|1|0,0,0,0,0,0|x".as_slice(),
            b"2|2026-09-01|0|0|1|0,0,0,0,0,0|",
            b"2|2026-09-01|0|1|1|2,0,0,0,0,0|",
            b"1|2026-09-01|0|1|1|x".as_slice(),
            b"1|2026-09-01|0|0|1|",
        ] {
            let mut game = Game::for_day("2026-09-01");
            assert!(!game.restore(bytes), "{bytes:?}");
            assert!(game.daily.is_empty());
            assert_eq!((game.played, game.wins), (0, 0));
        }
    }
    #[test]
    fn export_text_carries_the_board_and_statistics() {
        let mut game = Game::for_day("2026-09-01");
        let answer = game.answer().to_owned();
        let filler = if answer == "grape" { "adore" } else { "grape" };
        game.keyboard = Keyboard::with_text(filler);
        game.submit();
        game.keyboard = Keyboard::with_text(answer);
        game.submit();
        let text = game.export_text();
        assert!(text.contains("Inkling, September 1, 2026"), "{text}");
        assert!(text.contains("Solved in 2 of 6."), "{text}");
        assert!(text.contains("Played 1. Won 1."), "{text}");
        assert!(text.contains("Solved in 2: 1"), "{text}");
        assert!(
            text.contains('[') && text.contains("\u{00d7}") || text.contains('('),
            "{text}"
        );
    }
    #[test]
    fn clara_layout_is_clean() {
        let s = Game::for_day("2026-09-01").screen();
        let d = s.diagnostics(&CLARA_BW_METRICS, &Chrome::default());
        assert!(d.issues.is_empty(), "{:?}", d.issues);
    }
    #[test]
    fn archive_and_stats_and_typing_layouts_are_clean() {
        let mut game = Game::for_day("2026-09-01");
        game.view = View::Archive;
        let archive = game.screen();
        let d = archive.diagnostics(&CLARA_BW_METRICS, &Chrome::default());
        assert!(d.issues.is_empty(), "archive: {:?}", d.issues);
        let mut played = Game::for_day("2026-09-01");
        played.played = 6;
        played.wins = 6;
        played.dist = [1, 1, 1, 1, 1, 1];
        played.view = View::Stats;
        let stats = played.screen();
        let d = stats.diagnostics(&CLARA_BW_METRICS, &Chrome::default());
        assert!(d.issues.is_empty(), "stats: {:?}", d.issues);
        let mut typing = Game::for_day("2026-09-01");
        typing.typing = true;
        typing.daily = vec!["crane".into(), "stare".into()];
        let screen = typing.screen();
        let d = screen.diagnostics(&CLARA_BW_METRICS, &Chrome::default());
        assert!(d.issues.is_empty(), "typing: {:?}", d.issues);
    }
    #[test]
    fn how_to_play_is_short_and_reachable() {
        let mut game = Game::for_day("2026-09-01");
        let home = game.screen();
        assert!(home
            .layout_with(&CLARA_BW_METRICS, &Chrome::default())
            .rect_of_action(action_id("how-to-play"))
            .is_some());
        game.view = View::Help;
        let help = game.screen();
        assert!(help
            .diagnostics(&CLARA_BW_METRICS, &Chrome::measuring(true))
            .issues
            .is_empty());
    }
}

#[cfg(test)]
mod persistence_tests {
    use super::*;
    use kobo_sdk::AppRunner;

    #[test]
    fn saved_daily_game_and_statistics_survive_relaunch() {
        let mut game = Game::for_day("2026-09-01");
        game.hard = true;
        let answer = answer_for("2026-09-01");
        game.daily = vec!["crane".into(), answer.to_owned()];
        game.played = 4;
        game.wins = 3;
        game.dist = [0, 2, 1, 0, 0, 0];
        let bytes = game.encode();
        let mut restored = Game::for_day("2026-09-01");
        assert!(restored.restore(&bytes));
        assert!(restored.done());
        assert!(restored.hard);
        assert_eq!(restored.daily, game.daily);
        assert_eq!((restored.played, restored.wins), (4, 3));
        assert_eq!(restored.dist, [0, 2, 1, 0, 0, 0]);
        restored.submit();
        assert_eq!((restored.played, restored.wins), (4, 3));
        let mut tomorrow = Game::for_day("2026-09-02");
        assert!(tomorrow.restore(&bytes));
        assert!(tomorrow.daily.is_empty());
        assert_eq!((tomorrow.played, tomorrow.wins), (4, 3));
        assert_eq!(tomorrow.dist, [0, 2, 1, 0, 0, 0]);
    }

    #[test]
    fn input_waits_for_saved_state_and_duplicate_load_cannot_erase_edits() {
        let mut runner = AppRunner::new(Game::for_day("2026-09-01"));
        runner.start();
        runner.action(action_id("enter"));
        assert!(!runner.app().typing);
        runner.store_result(StoreResult::Loaded {
            key: STATE.into(),
            value: None,
        });
        runner.action(action_id("hard"));
        assert!(runner.app().hard);
        runner.store_result(StoreResult::Loaded {
            key: STATE.into(),
            value: Some(Game::for_day("2026-09-01").encode()),
        });
        assert!(runner.app().hard);
    }
}

#[cfg(test)]
mod help_layout_tests {
    use super::*;
    #[test]
    fn help_fits_supported_text_scales_and_geometries() {
        let mut game = Game::for_day("2026-09-01");
        game.view = View::Help;
        let screens = [game.screen()];
        for screen in screens {
            for (width, height, pixels_per_inch) in
                [(1072, 1448, 300), (758, 1024, 212), (1448, 1072, 300)]
            {
                for text_scale in kobo_ui::TextScale::STEPS {
                    let metrics = kobo_sdk::DisplayMetrics {
                        width,
                        height,
                        pixels_per_inch,
                        text_scale,
                    };
                    let chrome = kobo_ui::Chrome::measuring(true);
                    let diagnostics = screen.diagnostics(&metrics, &chrome);
                    assert!(
                        diagnostics.issues.is_empty(),
                        "{metrics:?}: {:?}",
                        diagnostics.issues
                    );
                    assert!(screen
                        .layout_with(&metrics, &chrome)
                        .rect_of_action(action_id("close-help"))
                        .is_some());
                }
            }
        }
    }
}
