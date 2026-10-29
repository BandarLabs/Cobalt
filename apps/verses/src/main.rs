//! Daily public-domain poetry with an offline shelf and `PoetryDB` search.
use kobo_sdk::clock::{Clock, Snapshot, SystemClock};
use kobo_sdk::keyboard::{TextEntry, Typing};
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Glyph, Header, KoboApp, Screen, ScreenBuilder,
    StoreResult, Task, TaskError, TaskId, TaskOutcome,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::hash::{Hash, Hasher};
use std::process::ExitCode;

const SETTINGS: &str = "settings";
/// The search box, which raises the reader's keyboard when it is tapped.
const SEARCH_FIELD: &str = "search-field";
const SEARCH_LIMIT: u32 = 512 * 1024;
const USER_AGENT: &str = "Cobalt Verses/0.1";

#[derive(Clone, Copy)]
struct Poem {
    /// What a favourite is recorded against.
    ///
    /// Favourites used to be the poem's position in this list, so adding a
    /// poem or changing the order silently moved every favourite onto a
    /// different poem. A name is what the reader chose; a position is an
    /// accident of how the shelf is written down.
    id: &'static str,
    title: &'static str,
    author: &'static str,
    year: u16,
    source: &'static str,
    /// The poem as it is set: a list of stanzas, each a list of lines.
    ///
    /// Poems used to be four lines each, which is a stanza of The Tiger and
    /// a third of Hope, offered with nothing to say it was not the poem. A
    /// reader who met them here met an excerpt believing it was the whole.
    /// These are complete, and the shape is stanzas rather than lines so the
    /// space between them is the poet's rather than the renderer's.
    stanzas: &'static [&'static [&'static str]],
    tags: &'static str,
}

impl Poem {
    #[cfg(test)]
    fn lines(self) -> impl Iterator<Item = &'static str> {
        self.stanzas
            .iter()
            .flat_map(|stanza| stanza.iter().copied())
    }

    #[cfg(test)]
    fn line_count(self) -> usize {
        self.stanzas.iter().map(|stanza| stanza.len()).sum()
    }
}

const CORPUS: &[Poem] = &[
    Poem {
        id: "dickinson-hope",
        title: "Hope",
        author: "Emily Dickinson",
        year: 1891,
        source: "Poems, Second Series, via Project Gutenberg",
        tags: "hope · a minute",
        stanzas: &[
            &[
                "Hope is the thing with feathers",
                "That perches in the soul,",
                "And sings the tune without the words,",
                "And never stops at all,",
            ],
            &[
                "And sweetest in the gale is heard;",
                "And sore must be the storm",
                "That could abash the little bird",
                "That kept so many warm.",
            ],
            &[
                "I've heard it in the chillest land,",
                "And on the strangest sea;",
                "Yet, never, in extremity,",
                "It asked a crumb of me.",
            ],
        ],
    },
    Poem {
        id: "blake-tiger",
        title: "The Tiger",
        author: "William Blake",
        year: 1794,
        source: "Songs of Innocence and of Experience, via Project Gutenberg",
        tags: "nature · two minutes",
        stanzas: &[
            &[
                "Tiger, tiger, burning bright",
                "In the forests of the night,",
                "What immortal hand or eye",
                "Could frame thy fearful symmetry?",
            ],
            &[
                "In what distant deeps or skies",
                "Burnt the fire of thine eyes?",
                "On what wings dare he aspire?",
                "What the hand dare seize the fire?",
            ],
            &[
                "And what shoulder and what art",
                "Could twist the sinews of thy heart?",
                "And, when thy heart began to beat,",
                "What dread hand and what dread feet?",
            ],
            &[
                "What the hammer? what the chain?",
                "In what furnace was thy brain?",
                "What the anvil? what dread grasp",
                "Dare its deadly terrors clasp?",
            ],
            &[
                "When the stars threw down their spears,",
                "And watered heaven with their tears,",
                "Did He smile His work to see?",
                "Did He who made the lamb make thee?",
            ],
            &[
                "Tiger, tiger, burning bright",
                "In the forests of the night,",
                "What immortal hand or eye",
                "Dare frame thy fearful symmetry?",
            ],
        ],
    },
    Poem {
        id: "shelley-ozymandias",
        title: "Ozymandias",
        author: "Percy Bysshe Shelley",
        year: 1818,
        source: "The Examiner, via Project Gutenberg",
        tags: "history · a minute",
        stanzas: &[&[
            "I met a traveller from an antique land",
            "Who said: Two vast and trunkless legs of stone",
            "Stand in the desert...Near them, on the sand,",
            "Half sunk, a shattered visage lies, whose frown,",
            "And wrinkled lip, and sneer of cold command,",
            "Tell that its sculptor well those passions read",
            "Which yet survive, stamped on these lifeless things,",
            "The hand that mocked them, and the heart that fed:",
            "And on the pedestal these words appear:",
            "\u{2018}My name is Ozymandias, king of kings:",
            "Look on my works, ye Mighty, and despair!\u{2019}",
            "Nothing beside remains. Round the decay",
            "Of that colossal wreck, boundless and bare",
            "The lone and level sands stretch far away.",
        ]],
    },
    Poem {
        id: "dickinson-chariot",
        title: "The Chariot",
        author: "Emily Dickinson",
        year: 1890,
        source: "Poems, First Series, via Project Gutenberg",
        tags: "time · two minutes",
        stanzas: &[
            &[
                "Because I could not stop for Death,",
                "He kindly stopped for me;",
                "The carriage held but just ourselves",
                "And Immortality.",
            ],
            &[
                "We slowly drove, he knew no haste,",
                "And I had put away",
                "My labor, and my leisure too,",
                "For his civility.",
            ],
            &[
                "We passed the school where children played,",
                "Their lessons scarcely done;",
                "We passed the fields of gazing grain,",
                "We passed the setting sun.",
            ],
            &[
                "We paused before a house that seemed",
                "A swelling of the ground;",
                "The roof was scarcely visible,",
                "The cornice but a mound.",
            ],
            &[
                "Since then 't is centuries; but each",
                "Feels shorter than the day",
                "I first surmised the horses' heads",
                "Were toward eternity.",
            ],
        ],
    },
    Poem {
        id: "shakespeare-sonnet-18",
        title: "Sonnet 18",
        author: "William Shakespeare",
        year: 1609,
        source: "Shake-speares Sonnets, via Project Gutenberg",
        tags: "love · a minute",
        stanzas: &[
            &[
                "Shall I compare thee to a summer's day?",
                "Thou art more lovely and more temperate:",
                "Rough winds do shake the darling buds of May,",
                "And summer's lease hath all too short a date;",
                "Sometime too hot the eye of heaven shines,",
                "And often is his gold complexion dimm'd;",
                "And every fair from fair sometime declines,",
                "By chance or nature's changing course untrimm'd;",
                "But thy eternal summer shall not fade,",
                "Nor lose possession of that fair thou ow'st;",
                "Nor shall Death brag thou wander'st in his shade,",
                "When in eternal lines to time thou grow'st:",
                "So long as men can breathe or eyes can see,",
                "So long lives this, and this gives life to thee.",
            ],
        ],
    },
    Poem {
        id: "shakespeare-sonnet-29",
        title: "Sonnet 29",
        author: "William Shakespeare",
        year: 1609,
        source: "Shake-speares Sonnets, via Project Gutenberg",
        tags: "love · a minute",
        stanzas: &[
            &[
                "When, in disgrace with fortune and men's eyes,",
                "I all alone beweep my outcast state,",
                "And trouble deaf heaven with my bootless cries,",
                "And look upon myself and curse my fate,",
                "Wishing me like to one more rich in hope,",
                "Featured like him, like him with friends possess'd,",
                "Desiring this man's art and that man's scope,",
                "With what I most enjoy contented least;",
                "Yet in these thoughts myself almost despising,",
                "Haply I think on thee, and then my state,",
                "Like to the lark at break of day arising",
                "From sullen earth, sings hymns at heaven's gate;",
                "For thy sweet love remember'd such wealth brings",
                "That then I scorn to change my state with kings.",
            ],
        ],
    },
    Poem {
        id: "shakespeare-sonnet-73",
        title: "Sonnet 73",
        author: "William Shakespeare",
        year: 1609,
        source: "Shake-speares Sonnets, via Project Gutenberg",
        tags: "age · a minute",
        stanzas: &[
            &[
                "That time of year thou mayst in me behold",
                "When yellow leaves, or none, or few, do hang",
                "Upon those boughs which shake against the cold,",
                "Bare ruin'd choirs, where late the sweet birds sang.",
                "In me thou see'st the twilight of such day",
                "As after sunset fadeth in the west,",
                "Which by and by black night doth take away,",
                "Death's second self, that seals up all in rest.",
                "In me thou see'st the glowing of such fire",
                "That on the ashes of his youth doth lie,",
                "As the death-bed whereon it must expire,",
                "Consumed with that which it was nourish'd by.",
                "This thou perceivest, which makes thy love more strong,",
                "To love that well which thou must leave ere long.",
            ],
        ],
    },
    Poem {
        id: "shakespeare-sonnet-116",
        title: "Sonnet 116",
        author: "William Shakespeare",
        year: 1609,
        source: "Shake-speares Sonnets, via Project Gutenberg",
        tags: "love · a minute",
        stanzas: &[
            &[
                "Let me not to the marriage of true minds",
                "Admit impediments. Love is not love",
                "Which alters when it alteration finds,",
                "Or bends with the remover to remove:",
                "O no! it is an ever-fixed mark",
                "That looks on tempests and is never shaken;",
                "It is the star to every wandering bark,",
                "Whose worth's unknown, although his height be taken.",
                "Love's not Time's fool, though rosy lips and cheeks",
                "Within his bending sickle's compass come;",
                "Love alters not with his brief hours and weeks,",
                "But bears it out even to the edge of doom.",
                "If this be error and upon me proved,",
                "I never writ, nor no man ever loved.",
            ],
        ],
    },
    Poem {
        id: "shakespeare-sonnet-130",
        title: "Sonnet 130",
        author: "William Shakespeare",
        year: 1609,
        source: "Shake-speares Sonnets, via Project Gutenberg",
        tags: "love · a minute",
        stanzas: &[
            &[
                "My mistress' eyes are nothing like the sun;",
                "Coral is far more red than her lips' red;",
                "If snow be white, why then her breasts are dun;",
                "If hairs be wires, black wires grow on her head.",
                "I have seen roses damask'd, red and white,",
                "But no such roses see I in her cheeks;",
                "And in some perfumes is there more delight",
                "Than in the breath that from my mistress reeks.",
                "I love to hear her speak, yet well I know",
                "That music hath a far more pleasing sound;",
                "I grant I never saw a goddess go;",
                "My mistress, when she walks, treads on the ground:",
                "And yet, by heaven, I think my love as rare",
                "As any she belied with false compare.",
            ],
        ],
    },
    Poem {
        id: "wordsworth-westminster",
        title: "Composed upon Westminster Bridge",
        author: "William Wordsworth",
        year: 1807,
        source: "Poems, in Two Volumes, via Project Gutenberg",
        tags: "cities · a minute",
        stanzas: &[
            &[
                "Earth has not anything to show more fair:",
                "Dull would he be of soul who could pass by",
                "A sight so touching in its majesty:",
                "This City now doth, like a garment, wear",
                "The beauty of the morning; silent, bare,",
                "Ships, towers, domes, theatres, and temples lie",
                "Open unto the fields, and to the sky;",
                "All bright and glittering in the smokeless air.",
                "Never did sun more beautifully steep",
                "In his first splendour, valley, rock, or hill;",
                "Ne'er saw I, never felt, a calm so deep!",
                "The river glideth at his own sweet will:",
                "Dear God! the very houses seem asleep;",
                "And all that mighty heart is lying still!",
            ],
        ],
    },
    Poem {
        id: "wordsworth-world",
        title: "The World Is Too Much with Us",
        author: "William Wordsworth",
        year: 1807,
        source: "Poems, in Two Volumes, via Project Gutenberg",
        tags: "nature · a minute",
        stanzas: &[
            &[
                "The world is too much with us; late and soon,",
                "Getting and spending, we lay waste our powers;",
                "Little we see in Nature that is ours;",
                "We have given our hearts away, a sordid boon!",
                "This Sea that bares her bosom to the moon;",
                "The winds that will be howling at all hours,",
                "And are up-gathered now like sleeping flowers;",
                "For this, for everything, we are out of tune;",
                "It moves us not. Great God! I'd rather be",
                "A Pagan suckled in a creed outworn;",
                "So might I, standing on this pleasant lea,",
                "Have glimpses that would make me less forlorn;",
                "Have sight of Proteus rising from the sea;",
                "Or hear old Triton blow his wreathed horn.",
            ],
        ],
    },
    Poem {
        id: "wordsworth-rainbow",
        title: "My Heart Leaps Up",
        author: "William Wordsworth",
        year: 1807,
        source: "Poems, in Two Volumes, via Project Gutenberg",
        tags: "nature · a minute",
        stanzas: &[
            &[
                "My heart leaps up when I behold",
                "A rainbow in the sky:",
                "So was it when my life began;",
                "So is it now I am a man;",
                "So be it when I shall grow old,",
                "Or let me die!",
                "The Child is father of the Man;",
                "And I could wish my days to be",
                "Bound each to each by natural piety.",
            ],
        ],
    },
    Poem {
        id: "wordsworth-lucy",
        title: "She Dwelt among the Untrodden Ways",
        author: "William Wordsworth",
        year: 1800,
        source: "Lyrical Ballads, via Project Gutenberg",
        tags: "loss · a minute",
        stanzas: &[
            &[
                "She dwelt among the untrodden ways",
                "Beside the springs of Dove,",
                "A Maid whom there were none to praise",
                "And very few to love:",
            ],
            &[
                "A violet by a mossy stone",
                "Half hidden from the eye!",
                "Fair as a star, when only one",
                "Is shining in the sky.",
            ],
            &[
                "She lived unknown, and few could know",
                "When Lucy ceased to be;",
                "But she is in her grave, and, oh,",
                "The difference to me!",
            ],
        ],
    },
    Poem {
        id: "keats-chapman",
        title: "On First Looking into Chapman's Homer",
        author: "John Keats",
        year: 1816,
        source: "Poems, via Project Gutenberg",
        tags: "discovery · a minute",
        stanzas: &[
            &[
                "Much have I travell'd in the realms of gold,",
                "And many goodly states and kingdoms seen;",
                "Round many western islands have I been",
                "Which bards in fealty to Apollo hold.",
                "Oft of one wide expanse had I been told",
                "That deep-brow'd Homer ruled as his demesne;",
                "Yet did I never breathe its pure serene",
                "Till I heard Chapman speak out loud and bold:",
                "Then felt I like some watcher of the skies",
                "When a new planet swims into his ken;",
                "Or like stout Cortez when with eagle eyes",
                "He star'd at the Pacific, and all his men",
                "Look'd at each other with a wild surmise,",
                "Silent, upon a peak in Darien.",
            ],
        ],
    },
    Poem {
        id: "keats-fears",
        title: "When I Have Fears",
        author: "John Keats",
        year: 1848,
        source: "Life, Letters, and Literary Remains, via Project Gutenberg",
        tags: "time · a minute",
        stanzas: &[
            &[
                "When I have fears that I may cease to be",
                "Before my pen has glean'd my teeming brain,",
                "Before high-piled books, in charactery,",
                "Hold like rich garners the full ripen'd grain;",
                "When I behold, upon the night's starr'd face,",
                "Huge cloudy symbols of a high romance,",
                "And think that I may never live to trace",
                "Their shadows, with the magic hand of chance;",
                "And when I feel, fair creature of an hour,",
                "That I shall never look upon thee more,",
                "Never have relish in the faery power",
                "Of unreflecting love; then on the shore",
                "Of the wide world I stand alone, and think",
                "Till love and fame to nothingness do sink.",
            ],
        ],
    },
    Poem {
        id: "rossetti-remember",
        title: "Remember",
        author: "Christina Rossetti",
        year: 1862,
        source: "Goblin Market and Other Poems, via Project Gutenberg",
        tags: "loss · a minute",
        stanzas: &[
            &[
                "Remember me when I am gone away,",
                "Gone far away into the silent land;",
                "When you can no more hold me by the hand,",
                "Nor I half turn to go yet turning stay.",
                "Remember me when no more day by day",
                "You tell me of our future that you plann'd:",
                "Only remember me; you understand",
                "It will be late to counsel then or pray.",
                "Yet if you should forget me for a while",
                "And afterwards remember, do not grieve:",
                "For if the darkness and corruption leave",
                "A vestige of the thoughts that once I had,",
                "Better by far you should forget and smile",
                "Than that you should remember and be sad.",
            ],
        ],
    },
    Poem {
        id: "rossetti-birthday",
        title: "A Birthday",
        author: "Christina Rossetti",
        year: 1862,
        source: "Goblin Market and Other Poems, via Project Gutenberg",
        tags: "joy · a minute",
        stanzas: &[
            &[
                "My heart is like a singing bird",
                "Whose nest is in a water'd shoot;",
                "My heart is like an apple-tree",
                "Whose boughs are bent with thickset fruit;",
                "My heart is like a rainbow shell",
                "That paddles in a halcyon sea;",
                "My heart is gladder than all these",
                "Because my love is come to me.",
            ],
            &[
                "Raise me a dais of silk and down;",
                "Hang it with vair and purple dyes;",
                "Carve it in doves and pomegranates,",
                "And peacocks with a hundred eyes;",
                "Work it in gold and silver grapes,",
                "In leaves and silver fleurs-de-lys;",
                "Because the birthday of my life",
                "Is come, my love is come to me.",
            ],
        ],
    },
    Poem {
        id: "rossetti-wind",
        title: "Who Has Seen the Wind?",
        author: "Christina Rossetti",
        year: 1872,
        source: "Sing-Song, via Project Gutenberg",
        tags: "nature · a minute",
        stanzas: &[
            &[
                "Who has seen the wind?",
                "Neither I nor you:",
                "But when the leaves hang trembling,",
                "The wind is passing through.",
            ],
            &[
                "Who has seen the wind?",
                "Neither you nor I:",
                "But when the trees bow down their heads,",
                "The wind is passing by.",
            ],
        ],
    },
    Poem {
        id: "blake-lamb",
        title: "The Lamb",
        author: "William Blake",
        year: 1789,
        source: "Songs of Innocence and of Experience, via Project Gutenberg",
        tags: "faith · a minute",
        stanzas: &[
            &[
                "Little Lamb, who made thee?",
                "Dost thou know who made thee?",
                "Gave thee life, and bid thee feed,",
                "By the stream and o'er the mead;",
                "Gave thee clothing of delight,",
                "Softest clothing, woolly, bright;",
                "Gave thee such a tender voice,",
                "Making all the vales rejoice?",
                "Little Lamb, who made thee?",
                "Dost thou know who made thee?",
            ],
            &[
                "Little Lamb, I'll tell thee,",
                "Little Lamb, I'll tell thee:",
                "He is called by thy name,",
                "For He calls Himself a Lamb.",
                "He is meek, and He is mild;",
                "He became a little child.",
                "I a child, and thou a lamb,",
                "We are called by His name.",
                "Little Lamb, God bless thee!",
                "Little Lamb, God bless thee!",
            ],
        ],
    },
    Poem {
        id: "blake-rose",
        title: "The Sick Rose",
        author: "William Blake",
        year: 1794,
        source: "Songs of Innocence and of Experience, via Project Gutenberg",
        tags: "nature · a minute",
        stanzas: &[
            &[
                "O Rose, thou art sick!",
                "The invisible worm",
                "That flies in the night,",
                "In the howling storm,",
            ],
            &[
                "Has found out thy bed",
                "Of crimson joy:",
                "And his dark secret love",
                "Does thy life destroy.",
            ],
        ],
    },
    Poem {
        id: "blake-sunflower",
        title: "Ah! Sun-flower",
        author: "William Blake",
        year: 1794,
        source: "Songs of Innocence and of Experience, via Project Gutenberg",
        tags: "longing · a minute",
        stanzas: &[
            &[
                "Ah, Sun-flower! weary of time,",
                "Who countest the steps of the Sun:",
                "Seeking after that sweet golden clime",
                "Where the traveller's journey is done:",
            ],
            &[
                "Where the Youth pined away with desire,",
                "And the pale Virgin shrouded in snow:",
                "Arise from their graves, and aspire",
                "Where my Sun-flower wishes to go.",
            ],
        ],
    },
    Poem {
        id: "burns-rose",
        title: "A Red, Red Rose",
        author: "Robert Burns",
        year: 1794,
        source: "The Poetical Works of Robert Burns, via Project Gutenberg",
        tags: "love · a minute",
        stanzas: &[
            &[
                "O my Luve's like a red, red rose,",
                "That's newly sprung in June:",
                "O my Luve's like the melodie,",
                "That's sweetly play'd in tune.",
            ],
            &[
                "As fair art thou, my bonnie lass,",
                "So deep in luve am I;",
                "And I will luve thee still, my dear,",
                "Till a' the seas gang dry.",
            ],
            &[
                "Till a' the seas gang dry, my dear,",
                "And the rocks melt wi' the sun;",
                "And I will luve thee still, my dear,",
                "While the sands o' life shall run.",
            ],
            &[
                "And fare thee weel, my only Luve!",
                "And fare thee weel a while!",
                "And I will come again, my Luve,",
                "Tho' it were ten thousand mile!",
            ],
        ],
    },
    Poem {
        id: "byron-beauty",
        title: "She Walks in Beauty",
        author: "Lord Byron",
        year: 1815,
        source: "Hebrew Melodies, via Project Gutenberg",
        tags: "love · a minute",
        stanzas: &[
            &[
                "She walks in beauty, like the night",
                "Of cloudless climes and starry skies;",
                "And all that's best of dark and bright",
                "Meet in her aspect and her eyes:",
                "Thus mellow'd to that tender light",
                "Which heaven to gaudy day denies.",
            ],
            &[
                "One shade the more, one ray the less,",
                "Had half impair'd the nameless grace",
                "Which waves in every raven tress,",
                "Or softly lightens o'er her face;",
                "Where thoughts serenely sweet express",
                "How pure, how dear their dwelling-place.",
            ],
            &[
                "And on that cheek, and o'er that brow,",
                "So soft, so calm, yet eloquent,",
                "The smiles that win, the tints that glow,",
                "But tell of days in goodness spent,",
                "A mind at peace with all below,",
                "A heart whose love is innocent!",
            ],
        ],
    },
    Poem {
        id: "tennyson-bar",
        title: "Crossing the Bar",
        author: "Alfred Tennyson",
        year: 1889,
        source: "Demeter and Other Poems, via Project Gutenberg",
        tags: "time · a minute",
        stanzas: &[
            &[
                "Sunset and evening star,",
                "And one clear call for me!",
                "And may there be no moaning of the bar,",
                "When I put out to sea,",
            ],
            &[
                "But such a tide as moving seems asleep,",
                "Too full for sound and foam,",
                "When that which drew from out the boundless deep",
                "Turns again home.",
            ],
            &[
                "Twilight and evening bell,",
                "And after that the dark!",
                "And may there be no sadness of farewell,",
                "When I embark;",
            ],
            &[
                "For tho' from out our bourne of Time and Place",
                "The flood may bear me far,",
                "I hope to see my Pilot face to face",
                "When I have crost the bar.",
            ],
        ],
    },
    Poem {
        id: "tennyson-eagle",
        title: "The Eagle",
        author: "Alfred Tennyson",
        year: 1851,
        source: "Poems, via Project Gutenberg",
        tags: "nature · a minute",
        stanzas: &[
            &[
                "He clasps the crag with crooked hands;",
                "Close to the sun in lonely lands,",
                "Ring'd with the azure world, he stands.",
            ],
            &[
                "The wrinkled sea beneath him crawls;",
                "He watches from his mountain walls,",
                "And like a thunderbolt he falls.",
            ],
        ],
    },
    Poem {
        id: "tennyson-break",
        title: "Break, Break, Break",
        author: "Alfred Tennyson",
        year: 1842,
        source: "Poems, via Project Gutenberg",
        tags: "loss · a minute",
        stanzas: &[
            &[
                "Break, break, break,",
                "On thy cold gray stones, O Sea!",
                "And I would that my tongue could utter",
                "The thoughts that arise in me.",
            ],
            &[
                "O well for the fisherman's boy,",
                "That he shouts with his sister at play!",
                "O well for the sailor lad,",
                "That he sings in his boat on the bay!",
            ],
            &[
                "And the stately ships go on",
                "To their haven under the hill;",
                "But O for the touch of a vanish'd hand,",
                "And the sound of a voice that is still!",
            ],
            &[
                "Break, break, break,",
                "At the foot of thy crags, O Sea!",
                "But the tender grace of a day that is dead",
                "Will never come back to me.",
            ],
        ],
    },
    Poem {
        id: "shelley-music",
        title: "Music, When Soft Voices Die",
        author: "Percy Bysshe Shelley",
        year: 1824,
        source: "Posthumous Poems, via Project Gutenberg",
        tags: "memory · a minute",
        stanzas: &[
            &[
                "Music, when soft voices die,",
                "Vibrates in the memory;",
                "Odours, when sweet violets sicken,",
                "Live within the sense they quicken.",
            ],
            &[
                "Rose leaves, when the rose is dead,",
                "Are heaped for the beloved's bed;",
                "And so thy thoughts, when thou art gone,",
                "Love itself shall slumber on.",
            ],
        ],
    },
    Poem {
        id: "stevenson-requiem",
        title: "Requiem",
        author: "Robert Louis Stevenson",
        year: 1887,
        source: "Underwoods, via Project Gutenberg",
        tags: "rest · a minute",
        stanzas: &[
            &[
                "Under the wide and starry sky,",
                "Dig the grave and let me lie.",
                "Glad did I live and gladly die,",
                "And I laid me down with a will.",
            ],
            &[
                "This be the verse you grave for me:",
                "Here he lies where he longed to be;",
                "Home is the sailor, home from sea,",
                "And the hunter home from the hill.",
            ],
        ],
    },
    Poem {
        id: "whitman-captain",
        title: "O Captain! My Captain!",
        author: "Walt Whitman",
        year: 1865,
        source: "Leaves of Grass, via Project Gutenberg",
        tags: "loss · two minutes",
        stanzas: &[
            &[
                "O Captain! my Captain! our fearful trip is done,",
                "The ship has weather'd every rack, the prize we sought is won,",
                "The port is near, the bells I hear, the people all exulting,",
                "While follow eyes the steady keel, the vessel grim and daring;",
                "But O heart! heart! heart!",
                "O the bleeding drops of red,",
                "Where on the deck my Captain lies,",
                "Fallen cold and dead.",
            ],
            &[
                "O Captain! my Captain! rise up and hear the bells;",
                "Rise up, for you the flag is flung, for you the bugle trills,",
                "For you bouquets and ribbon'd wreaths, for you the shores a-crowding,",
                "For you they call, the swaying mass, their eager faces turning;",
                "Here Captain! dear father!",
                "This arm beneath your head!",
                "It is some dream that on the deck,",
                "You've fallen cold and dead.",
            ],
            &[
                "My Captain does not answer, his lips are pale and still,",
                "My father does not feel my arm, he has no pulse nor will,",
                "The ship is anchor'd safe and sound, its voyage closed and done,",
                "From fearful trip the victor ship comes in with object won;",
                "Exult O shores, and ring O bells!",
                "But I with mournful tread,",
                "Walk the deck my Captain lies,",
                "Fallen cold and dead.",
            ],
        ],
    },
    Poem {
        id: "whitman-astronomer",
        title: "When I Heard the Learn'd Astronomer",
        author: "Walt Whitman",
        year: 1865,
        source: "Leaves of Grass, via Project Gutenberg",
        tags: "stars · a minute",
        stanzas: &[
            &[
                "When I heard the learn'd astronomer,",
                "When the proofs, the figures, were ranged in columns before me,",
                "When I was shown the charts and diagrams, to add, divide, and measure them,",
                "When I sitting heard the astronomer where he lectured with much applause in the lecture-room,",
                "How soon unaccountable I became tired and sick,",
                "Till rising and gliding out I wander'd off by myself,",
                "In the mystical moist night-air, and from time to time,",
                "Look'd up in perfect silence at the stars.",
            ],
        ],
    },
    Poem {
        id: "longfellow-arrow",
        title: "The Arrow and the Song",
        author: "Henry Wadsworth Longfellow",
        year: 1845,
        source: "The Belfry of Bruges and Other Poems, via Project Gutenberg",
        tags: "friendship · a minute",
        stanzas: &[
            &[
                "I shot an arrow into the air,",
                "It fell to earth, I knew not where;",
                "For, so swiftly it flew, the sight",
                "Could not follow it in its flight.",
            ],
            &[
                "I breathed a song into the air,",
                "It fell to earth, I knew not where;",
                "For who has sight so keen and strong,",
                "That it can follow the flight of song?",
            ],
            &[
                "Long, long afterward, in an oak",
                "I found the arrow, still unbroke;",
                "And the song, from beginning to end,",
                "I found again in the heart of a friend.",
            ],
        ],
    },
    Poem {
        id: "poe-eldorado",
        title: "Eldorado",
        author: "Edgar Allan Poe",
        year: 1849,
        source: "The Works of Edgar Allan Poe, via Project Gutenberg",
        tags: "quests · two minutes",
        stanzas: &[
            &[
                "Gaily bedight,",
                "A gallant knight,",
                "In sunshine and in shadow,",
                "Had journeyed long,",
                "Singing a song,",
                "In search of Eldorado.",
            ],
            &[
                "But he grew old,",
                "This knight so bold,",
                "And o'er his heart a shadow",
                "Fell as he found",
                "No spot of ground",
                "That looked like Eldorado.",
            ],
            &[
                "And, as his strength",
                "Failed him at length,",
                "He met a pilgrim shadow;",
                "\u{201c}Shadow,\u{201d} said he,",
                "\u{201c}Where can it be,",
                "This land of Eldorado?\u{201d}",
            ],
            &[
                "\u{201c}Over the Mountains",
                "Of the Moon,",
                "Down the Valley of the Shadow,",
                "Ride, boldly ride,\u{201d}",
                "The shade replied,",
                "\u{201c}If you seek for Eldorado!\u{201d}",
            ],
        ],
    },
    Poem {
        id: "donne-death",
        title: "Death, Be Not Proud",
        author: "John Donne",
        year: 1633,
        source: "Poems, via Project Gutenberg",
        tags: "faith · a minute",
        stanzas: &[
            &[
                "Death, be not proud, though some have called thee",
                "Mighty and dreadful, for thou art not so;",
                "For those whom thou think'st thou dost overthrow",
                "Die not, poor Death, nor yet canst thou kill me.",
                "From rest and sleep, which but thy pictures be,",
                "Much pleasure; then from thee much more must flow,",
                "And soonest our best men with thee do go,",
                "Rest of their bones, and soul's delivery.",
                "Thou art slave to fate, chance, kings, and desperate men,",
                "And dost with poison, war, and sickness dwell,",
                "And poppy or charms can make us sleep as well",
                "And better than thy stroke; why swell'st thou then?",
                "One short sleep past, we wake eternally",
                "And death shall be no more; Death, thou shalt die.",
            ],
        ],
    },
    Poem {
        id: "herrick-rosebuds",
        title: "To the Virgins, to Make Much of Time",
        author: "Robert Herrick",
        year: 1648,
        source: "Hesperides, via Project Gutenberg",
        tags: "time · a minute",
        stanzas: &[
            &[
                "Gather ye rosebuds while ye may,",
                "Old Time is still a-flying;",
                "And this same flower that smiles today",
                "Tomorrow will be dying.",
            ],
            &[
                "The glorious lamp of heaven, the sun,",
                "The higher he's a-getting,",
                "The sooner will his race be run,",
                "And nearer he's to setting.",
            ],
            &[
                "That age is best which is the first,",
                "When youth and blood are warmer;",
                "But being spent, the worse, and worst",
                "Times still succeed the former.",
            ],
            &[
                "Then be not coy, but use your time,",
                "And while ye may, go marry;",
                "For having lost but once your prime,",
                "You may forever tarry.",
            ],
        ],
    },
    Poem {
        id: "jonson-celia",
        title: "Song: To Celia",
        author: "Ben Jonson",
        year: 1616,
        source: "The Forest, via Project Gutenberg",
        tags: "love · a minute",
        stanzas: &[
            &[
                "Drink to me only with thine eyes,",
                "And I will pledge with mine;",
                "Or leave a kiss but in the cup,",
                "And I'll not look for wine.",
                "The thirst that from the soul doth rise",
                "Doth ask a drink divine;",
                "But might I of Jove's nectar sup,",
                "I would not change for thine.",
            ],
            &[
                "I sent thee late a rosy wreath,",
                "Not so much honouring thee",
                "As giving it a hope that there",
                "It could not withered be;",
                "But thou thereon didst only breathe,",
                "And sent'st it back to me;",
                "Since when it grows, and smells, I swear,",
                "Not of itself, but thee.",
            ],
        ],
    },
    Poem {
        id: "marlowe-shepherd",
        title: "The Passionate Shepherd to His Love",
        author: "Christopher Marlowe",
        year: 1600,
        source: "England's Helicon, via Project Gutenberg",
        tags: "love · two minutes",
        stanzas: &[
            &[
                "Come live with me and be my love,",
                "And we will all the pleasures prove,",
                "That valleys, groves, hills, and fields,",
                "Woods, or steepy mountain yields.",
            ],
            &[
                "And we will sit upon the rocks,",
                "Seeing the shepherds feed their flocks,",
                "By shallow rivers to whose falls",
                "Melodious birds sing madrigals.",
            ],
            &[
                "And I will make thee beds of roses",
                "And a thousand fragrant posies,",
                "A cap of flowers, and a kirtle",
                "Embroidered all with leaves of myrtle;",
            ],
            &[
                "A gown made of the finest wool",
                "Which from our pretty lambs we pull;",
                "Fair lined slippers for the cold,",
                "With buckles of the purest gold;",
            ],
            &[
                "A belt of straw and ivy buds,",
                "With coral clasps and amber studs:",
                "And if these pleasures may thee move,",
                "Come live with me, and be my love.",
            ],
            &[
                "The shepherds' swains shall dance and sing",
                "For thy delight each May morning:",
                "If these delights thy mind may move,",
                "Then live with me and be my love.",
            ],
        ],
    },
    Poem {
        id: "hunt-jenny",
        title: "Jenny Kiss'd Me",
        author: "Leigh Hunt",
        year: 1838,
        source: "The Monthly Chronicle, via Project Gutenberg",
        tags: "joy · a minute",
        stanzas: &[
            &[
                "Jenny kiss'd me when we met,",
                "Jumping from the chair she sat in;",
                "Time, you thief, who love to get",
                "Sweets into your list, put that in!",
                "Say I'm weary, say I'm sad,",
                "Say that health and wealth have miss'd me,",
                "Say I'm growing old, but add,",
                "Jenny kiss'd me.",
            ],
        ],
    },
    Poem {
        id: "browning-count",
        title: "How Do I Love Thee?",
        author: "Elizabeth Barrett Browning",
        year: 1850,
        source: "Sonnets from the Portuguese, via Project Gutenberg",
        tags: "love · a minute",
        stanzas: &[
            &[
                "How do I love thee? Let me count the ways.",
                "I love thee to the depth and breadth and height",
                "My soul can reach, when feeling out of sight",
                "For the ends of Being and ideal Grace.",
                "I love thee to the level of everyday's",
                "Most quiet need, by sun and candle-light.",
                "I love thee freely, as men strive for Right;",
                "I love thee purely, as they turn from Praise.",
                "I love thee with the passion put to use",
                "In my old griefs, and with my childhood's faith.",
                "I love thee with a love I seemed to lose",
                "With my lost saints, I love thee with the breath,",
                "Smiles, tears, of all my life! and, if God choose,",
                "I shall but love thee better after death.",
            ],
        ],
    },
    Poem {
        id: "dickinson-success",
        title: "Success Is Counted Sweetest",
        author: "Emily Dickinson",
        year: 1878,
        source: "Poems, First Series, via Project Gutenberg",
        tags: "longing · a minute",
        stanzas: &[
            &[
                "Success is counted sweetest",
                "By those who ne'er succeed.",
                "To comprehend a nectar",
                "Requires sorest need.",
            ],
            &[
                "Not one of all the purple host",
                "Who took the flag to-day",
                "Can tell the definition,",
                "So clear, of victory,",
            ],
            &[
                "As he, defeated, dying,",
                "On whose forbidden ear",
                "The distant strains of triumph",
                "Break, agonized and clear!",
            ],
        ],
    },
    Poem {
        id: "dickinson-moor",
        title: "I Never Saw a Moor",
        author: "Emily Dickinson",
        year: 1890,
        source: "Poems, First Series, via Project Gutenberg",
        tags: "faith · a minute",
        stanzas: &[
            &[
                "I never saw a moor,",
                "I never saw the sea;",
                "Yet know I how the heather looks,",
                "And what a wave must be.",
            ],
            &[
                "I never spoke with God,",
                "Nor visited in heaven;",
                "Yet certain am I of the spot",
                "As if the chart were given.",
            ],
        ],
    },
    Poem {
        id: "lear-owl",
        title: "The Owl and the Pussy-cat",
        author: "Edward Lear",
        year: 1871,
        source: "Nonsense Songs, Stories, Botany, and Alphabets, via Project Gutenberg",
        tags: "nonsense · two minutes",
        stanzas: &[
            &[
                "The Owl and the Pussy-cat went to sea",
                "In a beautiful pea-green boat,",
                "They took some honey, and plenty of money,",
                "Wrapped up in a five-pound note.",
                "The Owl looked up to the stars above,",
                "And sang to a small guitar,",
                "\u{201c}O lovely Pussy! O Pussy, my love,",
                "What a beautiful Pussy you are,",
                "You are,",
                "You are!",
                "What a beautiful Pussy you are!\u{201d}",
            ],
            &[
                "Pussy said to the Owl, \u{201c}You elegant fowl!",
                "How charmingly sweet you sing!",
                "O let us be married! too long we have tarried:",
                "But what shall we do for a ring?\u{201d}",
                "They sailed away, for a year and a day,",
                "To the land where the Bong-tree grows,",
                "And there in a wood a Piggy-wig stood,",
                "With a ring at the end of his nose,",
                "His nose,",
                "His nose,",
                "With a ring at the end of his nose.",
            ],
            &[
                "\u{201c}Dear Pig, are you willing to sell for one shilling",
                "Your ring?\u{201d} Said the Piggy, \u{201c}I will.\u{201d}",
                "So they took it away, and were married next day",
                "By the Turkey who lives on the hill.",
                "They dined on mince, and slices of quince,",
                "Which they ate with a runcible spoon;",
                "And hand in hand, on the edge of the sand,",
                "They danced by the light of the moon,",
                "The moon,",
                "The moon,",
                "They danced by the light of the moon.",
            ],
        ],
    },
];

#[derive(Clone, Debug, Deserialize, Serialize, Hash)]
struct OnlinePoem {
    title: String,
    author: String,
    #[serde(default)]
    lines: Vec<String>,
    #[serde(default, deserialize_with = "count_text")]
    linecount: String,
}

/// `PoetryDB` sends a poem's line count as a string for some poems and as a
/// number for others, Emily Dickinson's among them. Read as a string only, one
/// numeric count failed the whole list, so searching for the most read poet in
/// the collection reported that it held nothing by her.
fn count_text<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Count {
        Text(String),
        Number(u64),
    }
    Ok(match Count::deserialize(deserializer)? {
        Count::Text(text) => text,
        Count::Number(number) => number.to_string(),
    })
}

/// Orders titles the way a reader counts: "Sonnet 2" before "Sonnet 10",
/// where a plain comparison of the text put every sonnet in the teens between
/// the first and the second.
fn natural_order(a: &str, b: &str) -> std::cmp::Ordering {
    fn chunks(text: &str) -> Vec<(bool, &str)> {
        let mut chunks = Vec::new();
        let mut start = 0;
        let mut digits = None;
        for (index, character) in text.char_indices() {
            let digit = character.is_ascii_digit();
            if digits != Some(digit) {
                if let Some(was) = digits {
                    chunks.push((was, &text[start..index]));
                }
                start = index;
                digits = Some(digit);
            }
        }
        if let Some(was) = digits {
            chunks.push((was, &text[start..]));
        }
        chunks
    }
    // Quotation marks and apostrophes that open a title are not where it
    // files: "Whose are the little beds" belongs under W.
    fn file(text: &str) -> &str {
        text.trim_start_matches(|c: char| !c.is_alphanumeric())
    }
    let (left, right) = (chunks(file(a)), chunks(file(b)));
    for ((left_digits, left), (right_digits, right)) in left.iter().zip(&right) {
        let order = if *left_digits && *right_digits {
            let (l, r) = (left.trim_start_matches('0'), right.trim_start_matches('0'));
            l.len().cmp(&r.len()).then_with(|| l.cmp(r))
        } else {
            left.to_lowercase().cmp(&right.to_lowercase())
        };
        if order != std::cmp::Ordering::Equal {
            return order;
        }
    }
    left.len().cmp(&right.len())
}

/// Where a search looks: everywhere, or in one field.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Scope {
    #[default]
    Anything,
    Title,
    Poet,
    Line,
}

impl Scope {
    const ALL: [(Self, &'static str, &'static str); 4] = [
        (Self::Anything, "scope-any", "Anything"),
        (Self::Title, "scope-title", "Title"),
        (Self::Poet, "scope-poet", "Poet"),
        (Self::Line, "scope-line", "Line"),
    ];

    const fn fields(self) -> &'static str {
        match self {
            Self::Anything => "author,title,lines",
            Self::Title => "title",
            Self::Poet => "author",
            Self::Line => "lines",
        }
    }
}

/// Poets to start from, offered while the search box is empty. All of them
/// are well represented in `PoetryDB`, so a tap never lands on an empty page.
const SUGGESTED_POETS: [&str; 6] = [
    "Emily Dickinson",
    "William Blake",
    "John Keats",
    "Christina Rossetti",
    "Walt Whitman",
    "William Shakespeare",
];

/// "a minute", "three minutes": how long a poem takes, at a reading pace of
/// about twenty lines a minute aloud.
fn reading_time(lines: &str) -> String {
    let lines = lines.trim().parse::<u32>().unwrap_or(0);
    match lines.div_ceil(20).max(1) {
        1 => "a minute".to_owned(),
        2 => "two minutes".to_owned(),
        3 => "three minutes".to_owned(),
        4 => "four minutes".to_owned(),
        minutes => format!("{minutes} minutes"),
    }
}

#[derive(Clone, Debug)]
struct OnlineLine {
    text: String,
    stanza_start: bool,
}

type OnlinePages = Vec<Vec<OnlineLine>>;
/// A bundled poem's pages, with the poem and panel they were measured for and
/// the size its verse is set at.
type PoemLayout = (
    usize,
    kobo_ui::DisplayMetrics,
    kobo_ui::TextScale,
    Vec<Vec<VerseRun>>,
);
type OnlineLayout = (kobo_sdk::DisplayMetrics, u64, OnlinePages);

#[derive(Clone)]
struct PoemRow {
    section: Option<String>,
    action: String,
    title: String,
    detail: String,
    glyph: Glyph,
}

#[derive(Default, Deserialize, Serialize)]
struct Saved {
    /// Poems from the shelf, by name.
    #[serde(default)]
    favorites: BTreeSet<String>,
    online_favorites: Vec<OnlinePoem>,
    /// Written by a sleep-screen setting that never reached the sleep screen.
    ///
    /// The runtime has no way for an application to draw while the reader
    /// sleeps, so the setting was a switch wired to nothing and is gone. The
    /// field stays so a reader who goes back to an earlier version still has
    /// their favourites read back.
    #[serde(default)]
    sleep: bool,
    /// Today's poem from `PoetryDB`, and the day it was chosen for.
    #[serde(default)]
    daily: Option<DailyPoem>,
}

/// A poem fetched for one day, kept so the day's poem is the same poem all
/// day and still there when the reader goes offline.
#[derive(Clone, Debug, Deserialize, Serialize)]
struct DailyPoem {
    day: (u16, u8, u8),
    poem: OnlinePoem,
}

/// Lengths a daily poem is drawn from, so it is a poem for a page or two and
/// never one of `PoetryDB`'s thousand line epics.
const DAILY_LENGTHS: [u16; 5] = [12, 14, 16, 20, 24];

/// A ceiling on what is accepted as a daily poem, whatever the service sends.
const MAX_DAILY_LINES: usize = 60;

/// Asks `PoetryDB` for one poem of a readable length. A random one, so each
/// reader meets a different poem from the thousands it holds rather than the
/// same few bundled ones on rotation.
fn daily_task(day: (u16, u8, u8)) -> Task {
    let (year, month, date) = day;
    let length = DAILY_LENGTHS[daily_index(year, month, date) % DAILY_LENGTHS.len()];
    Task::Fetch {
        url: format!(
            "https://poetrydb.org/linecount,random/{length};1/author,title,lines,linecount"
        ),
        offset: 0,
        max_bytes: SEARCH_LIMIT,
        credential: None,
        headers: vec![Header::new("User-Agent", USER_AGENT)],
    }
}

/// What the shelf wrote before poems had names: favourites as positions.
///
/// Read only when the current form will not decode, which is exactly when the
/// stored favourites are numbers. Written back by name, so this is read once
/// per reader and then never again.
#[derive(Default, Deserialize)]
struct LegacySaved {
    #[serde(default)]
    favorites: BTreeSet<usize>,
    #[serde(default)]
    online_favorites: Vec<OnlinePoem>,
    #[serde(default)]
    sleep: bool,
}

/// The air between two stanzas, in hundredths of an em: three quarters of a
/// line, which is what a printed book of verse leaves.
const STANZA_AIR: u16 = 75;

/// Verse leading, as a percentage of the reading face's line height.
const VERSE_LEADING: u16 = 88;

/// The size verse is set at: one step above whatever the reader chose.
///
/// A poem is a page looked at rather than moved through, and these are short
/// lines with wide margins either side, so verse carries a step more than
/// prose would. It is a step above the reader's own setting rather than a
/// fixed size, because a fixed one is an override: a reader who asked for the
/// smallest type got three steps more than they asked for, and at that size
/// a sonnet no longer fits the panel it is measured against.
///
/// Pagination is measured at this size too. A page measured at one size and
/// set at another loses its last lines.
///
/// The step is given back when it is the only thing splitting a short poem:
/// see [`Verses::poem_pages`].
fn poem_scale(context: &Context) -> kobo_ui::TextScale {
    let chosen = context.metrics().text_scale;
    chosen.larger().unwrap_or(chosen)
}

/// The lines of one stanza that belong on one page.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct VerseRun {
    stanza: usize,
    from: usize,
    to: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum View {
    Today,
    /// The card being written out to a paired computer.
    Card,
    Browse,
    Reading,
    Search,
    Results,
    Online,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Pending {
    Search,
    Open(usize),
}

struct Verses {
    view: View,
    poem: usize,
    online: Option<usize>,
    online_page: usize,
    online_layout: std::cell::RefCell<Option<OnlineLayout>>,
    /// The pages of the bundled poem last measured, and what they were
    /// measured for. Paging lays each page out to see what fits, which is
    /// worth doing once per poem and panel rather than on every repaint of a
    /// single core reader.
    poem_layout: std::cell::RefCell<Option<PoemLayout>>,
    browse_page: usize,
    results_page: usize,
    online_from: View,
    card_from: View,
    saved: Saved,
    /// The words of a search, typed on the reader's own keyboard.
    entry: TextEntry,
    results: Vec<OnlinePoem>,
    /// Where the next search looks, and what the last one asked for.
    scope: Scope,
    query: String,
    task: Option<TaskId>,
    /// The fetch for today's poem, separate from searches so a search does
    /// not cancel it and it does not hold up a search.
    daily_task: Option<TaskId>,
    pending: Option<Pending>,
    notice: Option<String>,
    loaded: bool,
    /// Which page of the poem is open. A long poem is more than one panel.
    poem_page: usize,
    /// The reader's own day, as the poem on the Today screen was chosen for.
    ///
    /// Kept so that crossing midnight with the application open moves the
    /// Today screen on to the next day's poem rather than leaving yesterday's
    /// there until it is restarted.
    day: Option<(u16, u8, u8)>,
    /// A card on its way to a paired computer.
    export: Option<kobo_sdk::exports::Export>,
    /// Whether a favourite is still waiting for storage to confirm it.
    ///
    /// Marking a poem used to write and walk away. A write that failed took
    /// the favourite with it and said nothing, so a reader found their poem
    /// unmarked the next time they opened the shelf and had no idea why.
    saving: bool,
}

impl Default for Verses {
    fn default() -> Self {
        Self {
            view: View::Today,
            poem: poem_for_today(reader_clock().as_ref()),
            online: None,
            online_page: 0,
            online_layout: std::cell::RefCell::new(None),
            poem_layout: std::cell::RefCell::new(None),
            browse_page: 0,
            results_page: 0,
            online_from: View::Results,
            card_from: View::Today,
            saved: Saved::default(),
            entry: TextEntry::new().opened_by(SEARCH_FIELD),
            results: Vec::new(),
            scope: Scope::Anything,
            query: String::new(),
            task: None,
            daily_task: None,
            pending: None,
            notice: None,
            loaded: false,
            poem_page: 0,
            export: None,
            day: reader_clock()
                .now()
                .ok()
                .and_then(Snapshot::date)
                .map(|date| (date.year, date.month, date.day)),
            saving: false,
        }
    }
}

/// The name a poet is filed under: the last word of how they are credited.
fn surname(author: &str) -> &str {
    author.rsplit(' ').next().unwrap_or(author)
}

fn daily_index(year: u16, month: u8, day: u8) -> usize {
    ((year as usize * 372) + (month as usize * 31) + day as usize) % CORPUS.len()
}

/// The poem for the day the reader is actually holding the device on.
///
/// This was the first of September 2026, written into the default state, so an
/// application called "daily poetry" offered the same poem for ever. The
/// platform's clock is injectable and carries an explicit offset rather than a
/// guessed time zone, so the day here is the reader's own day and a test can
/// put the device on either side of midnight.
fn poem_for_today(clock: &dyn Clock) -> usize {
    clock
        .now()
        .ok()
        .and_then(Snapshot::date)
        .map_or(0, |date| daily_index(date.year, date.month, date.day))
}

/// The reader's clock, on the reader's own day.
///
/// The SDK's, which takes the offset the runtime hands every application and
/// never fails, so the day a poem is chosen for is the day on the status bar.
fn reader_clock() -> Box<dyn Clock> {
    Box::new(SystemClock::reader())
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(byte));
            }
            _ => {
                let _ = write!(out, "%{byte:02X}");
            }
        }
    }
    out
}

fn search_task(query: &str, scope: Scope) -> Task {
    let fields = scope.fields();
    Task::Fetch {
        url: format!(
            "https://poetrydb.org/{fields}/{}/author,title,linecount",
            escape(query)
        ),
        offset: 0,
        max_bytes: SEARCH_LIMIT,
        credential: None,
        headers: vec![Header::new("User-Agent", USER_AGENT)],
    }
}

fn poem_task(title: &str) -> Task {
    Task::Fetch {
        url: format!(
            "https://poetrydb.org/title/{}:abs/author,title,lines,linecount",
            escape(title)
        ),
        offset: 0,
        max_bytes: SEARCH_LIMIT,
        credential: None,
        headers: vec![Header::new("User-Agent", USER_AGENT)],
    }
}

impl Verses {
    /// One page of a poem, set the way a poem is set.
    ///
    /// The title and the poet head the first page in the display serif, and
    /// the poem follows flush left in the book face, a stanza to a paragraph
    /// with the poet's space between stanzas and none between their lines.
    /// Where it came from closes the last page. A poem that fits on one page
    /// sits in the middle of it: hung from the top it left half the panel
    /// empty beneath twelve lines, which read as a page that had failed to
    /// load the rest.
    fn local_poem(&self, context: &Context) -> Screen {
        let (scale, pages) = self.poem_pages(context);
        let page = self.poem_page.min(pages.len().saturating_sub(1));
        self.poem_page_screen(&pages[page], page, pages.len(), scale)
    }

    fn poem_page_screen(
        &self,
        runs: &[VerseRun],
        page: usize,
        count: usize,
        scale: kobo_ui::TextScale,
    ) -> Screen {
        let poem = CORPUS[self.poem];
        let mut screen = ScreenBuilder::new("verses-poem").top_bar(if self.view == View::Today {
            "Today's poem"
        } else {
            poem.author
        });
        if self.view == View::Today {
            screen = screen.top_bar_glyph("browse", "Browse", Glyph::Grid);
        }
        screen = screen.top_bar_glyph(
            "favorite",
            if self.saved.favorites.contains(CORPUS[self.poem].id) {
                "Remove favorite"
            } else {
                "Favorite"
            },
            Glyph::Heart,
        );
        let centred = count == 1;
        if centred {
            screen = screen.fill();
        }
        if page == 0 {
            screen = screen
                .heading_at_level(2, poem.title)
                .secondary(format!("{} · {}", poem.author, poem.year));
        }
        screen = screen.reading(true).text_scale(scale);
        screen = set_verse(screen, poem, runs);
        if page + 1 == count {
            screen = screen.secondary(format!("From {}", poem.source));
        }
        if centred {
            screen = screen.fill();
        }
        if count > 1 {
            screen = screen
                .page_turns("poem-previous", "poem-next")
                .page_position(
                    u16::try_from(page + 1).unwrap_or(1),
                    u16::try_from(count).unwrap_or(1),
                );
        }
        screen.bottom_action("card", "Make a quote card").build()
    }

    /// Which lines go on which page, measured against the panel, and the size
    /// they are set at.
    ///
    /// Each page is filled by laying it out and keeping whatever still fits,
    /// a whole stanza at a time, so the measure is the page the reader will
    /// see rather than an estimate of it. The estimate this replaced held a
    /// line back for the attribution, charged every gap between stanzas a
    /// whole line, and moved a stanza that did not fit the remainder to the
    /// next page: Hope, twelve lines in three stanzas, came out as two pages
    /// with four lines on the second and the bottom half of the first empty.
    ///
    /// A poem that fits one page is measured as the one page it is, without
    /// the page-position strip a longer poem carries. Measured as the first
    /// page of two, a sonnet lost its closing couplet to a strip it would
    /// never have had, and read eleven lines and then three. If the poem fits
    /// only at the reader's own size rather than a step above it, it is set
    /// at their size: a sonnet whole is worth more than a sonnet large.
    ///
    /// A stanza is only split when it is taller than a page on its own, which
    /// a long one is at the largest text sizes, and then line by line so
    /// nothing is lost.
    fn poem_pages(&self, context: &Context) -> (kobo_ui::TextScale, Vec<Vec<VerseRun>>) {
        let metrics = context.metrics();
        if let Some((poem, measured, scale, pages)) = self.poem_layout.borrow().as_ref() {
            if *poem == self.poem && *measured == metrics {
                return (*scale, pages.clone());
            }
        }
        let (scale, pages) = self.measure_poem_pages(context);
        *self.poem_layout.borrow_mut() = Some((self.poem, metrics, scale, pages.clone()));
        (scale, pages)
    }

    fn page_fits(context: &Context, screen: &Screen) -> bool {
        screen
            .diagnostics(&context.metrics(), &kobo_sdk::Chrome::measuring(true))
            .issues
            .is_empty()
    }

    fn measure_poem_pages(&self, context: &Context) -> (kobo_ui::TextScale, Vec<Vec<VerseRun>>) {
        let poem = CORPUS[self.poem];
        let whole: Vec<VerseRun> = poem
            .stanzas
            .iter()
            .enumerate()
            .map(|(stanza, lines)| VerseRun {
                stanza,
                from: 0,
                to: lines.len(),
            })
            .collect();
        let larger = poem_scale(context);
        let own = context.metrics().text_scale;
        for scale in [larger, own] {
            if Self::page_fits(context, &self.poem_page_screen(&whole, 0, 1, scale)) {
                return (scale, vec![whole]);
            }
        }
        let scale = larger;
        // Measured as a page of a longer poem, with the page position and the
        // attribution both present, so a page that fits here fits wherever it
        // lands.
        let fits = |runs: &[VerseRun], first: bool| {
            Self::page_fits(
                context,
                &self.poem_page_screen(runs, usize::from(!first), 2, scale),
            )
        };
        let mut pages: Vec<Vec<VerseRun>> = Vec::new();
        let mut current: Vec<VerseRun> = Vec::new();
        for (stanza, lines) in poem.stanzas.iter().enumerate() {
            let mut from = 0;
            while from < lines.len() {
                let first = pages.is_empty();
                let mut whole = current.clone();
                whole.push(VerseRun {
                    stanza,
                    from,
                    to: lines.len(),
                });
                if fits(&whole, first) {
                    current = whole;
                    break;
                }
                if !current.is_empty() {
                    pages.push(std::mem::take(&mut current));
                    continue;
                }
                // Alone on a page and still too tall: take as many lines as
                // fit, and never fewer than one, so the poem always moves on.
                let mut to = from + 1;
                while to < lines.len()
                    && fits(
                        &[VerseRun {
                            stanza,
                            from,
                            to: to + 1,
                        }],
                        first,
                    )
                {
                    to += 1;
                }
                // Shared out evenly over the pages it needs, rather than
                // filling the first and leaving the last with a couplet. Every
                // share is no longer than what was just measured to fit.
                let left = lines.len() - from;
                let share = left.div_ceil(left.div_ceil(to - from));
                let to = from + share;
                pages.push(vec![VerseRun { stanza, from, to }]);
                from = to;
            }
        }
        if !current.is_empty() {
            pages.push(current);
        }
        if pages.is_empty() {
            pages.push(vec![VerseRun {
                stanza: 0,
                from: 0,
                to: poem.stanzas.first().map_or(0, |lines| lines.len()),
            }]);
        }
        (scale, pages)
    }

    fn online_prefix(&self, context: &Context, poem: &OnlinePoem, page: usize) -> ScreenBuilder {
        let favorite = self
            .saved
            .online_favorites
            .iter()
            .any(|saved| saved.title == poem.title && saved.author == poem.author);
        let today = self.view == View::Today;
        let mut screen = ScreenBuilder::new("verses-online").top_bar(if today {
            "Today's poem".to_owned()
        } else {
            context.clamped_row(&poem.author, 1, false)
        });
        screen = if today {
            screen.top_bar_glyph("browse", "Browse", Glyph::Grid)
        } else {
            screen.top_bar_glyph("more-by-author", "More by this poet", Glyph::Person)
        };
        screen = screen.top_bar_glyph(
            "favorite",
            if favorite {
                "Remove favorite"
            } else {
                "Favorite"
            },
            Glyph::Heart,
        );
        if let Some(notice) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, notice);
        }
        if page == 0 {
            // The same head as a bundled poem: its title in the display serif
            // and who wrote it beneath, instead of the title squeezed into the
            // bar and the poet set as the first line of the page.
            screen = screen
                .heading_at_level(2, context.clamped_row(&poem.title, 2, true))
                .secondary(context.clamped_row(&poem.author, 2, false));
        }
        screen.reading(true)
    }

    fn online_page_screen(
        &self,
        context: &Context,
        poem: &OnlinePoem,
        lines: &[OnlineLine],
        page: usize,
        count: usize,
    ) -> Screen {
        let mut screen = self.online_prefix(context, poem, page);
        screen = set_online_verse(screen, lines);
        if page + 1 == count {
            screen = screen.secondary("From PoetryDB");
        }
        if count > 1 {
            screen = screen
                .page_turns("online-previous", "online-next")
                .page_position(
                    u16::try_from(page + 1).unwrap_or(u16::MAX),
                    u16::try_from(count).unwrap_or(u16::MAX),
                );
        }
        // A fetched poem makes a card as a bundled one does: today's poem
        // used to lose the card the day it began to come from PoetryDB.
        screen.bottom_action("card", "Make a quote card").build()
    }

    /// Keep verse lines and stanza gaps, measuring the same rich-text nodes
    /// the reader sees. Cache the result so each page turn stays cheap.
    ///
    /// A poem that fits one page is one page, measured as that page rather
    /// than as the first of several. Otherwise a stanza moves to the next
    /// page whole when it does not fit the rest of this one, and is cut line
    /// by line only when it is taller than a page by itself. Filling every
    /// page to the last line it could hold set the closing couplet of a
    /// sonnet on a page of its own.
    fn online_pages(&self, context: &Context, poem: &OnlinePoem) -> OnlinePages {
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        poem.hash(&mut hash);
        self.notice.hash(&mut hash);
        let key = hash.finish();
        if let Some((metrics, saved_key, pages)) = self.online_layout.borrow().as_ref() {
            if *metrics == context.metrics() && *saved_key == key {
                return pages.clone();
            }
        }
        let mut pending = std::collections::VecDeque::new();
        let mut stanza_start = false;
        for text in &poem.lines {
            if text.trim().is_empty() {
                stanza_start = true;
                continue;
            }
            pending.push_back(OnlineLine {
                text: text.clone(),
                stanza_start,
            });
            stanza_start = false;
        }
        let metrics = context.metrics();
        let all: Vec<OnlineLine> = pending.iter().cloned().collect();
        if !all.is_empty()
            && Self::page_fits(context, &self.online_page_screen(context, poem, &all, 0, 1))
        {
            let pages = vec![all];
            *self.online_layout.borrow_mut() = Some((metrics, key, pages.clone()));
            return pages;
        }
        // Measured with the first page's heading and the attribution, so every
        // page has at least the room the fullest one needs.
        let prefix = self.online_page_screen(context, poem, &[], 0, 2);
        let chrome =
            kobo_ui::Chrome::for_screen(&prefix, false, kobo_ui::Chrome::measuring(true).status);
        let prefix_height = prefix.layout_with(&metrics, &chrome).content_used();
        let (capacity, gap) = kobo_ui::with_text_scale(metrics.text_scale, || {
            // Reading screens have no status strip. Reserve the same page
            // position band as the renderer, then the measured author/notice.
            // The card action takes the foot of the panel, as a bar would.
            let area = metrics.prose_area_in(true, true, kobo_ui::Face::Reading);
            (
                area.height
                    .saturating_sub(metrics.page_position_band())
                    .saturating_sub(prefix_height),
                area.gap,
            )
        });
        // Measure each source line once, rather than relaying out the entire
        // growing page for every additional line.
        let measure = |line: &OnlineLine| measure_online_line(line, &metrics, &chrome);
        let mut pages = Vec::new();
        let mut page = Vec::new();
        let mut used = 0;
        while let Some(mut line) = pending.pop_front() {
            if page.is_empty() {
                line.stanza_start = false;
            } else if line.stanza_start {
                // The whole stanza, as it would sit at the top of a page.
                let mut stanza = vec![OnlineLine {
                    text: line.text.clone(),
                    stanza_start: false,
                }];
                stanza.extend(
                    pending
                        .iter()
                        .take_while(|next| !next.stanza_start)
                        .cloned(),
                );
                let alone = stanza.iter().map(|line| measure(line).0 + gap).sum::<i32>();
                let here = alone + measure(&line).0 - measure(&stanza[0]).0;
                if used + here > capacity && alone <= capacity {
                    pages.push(std::mem::take(&mut page));
                    used = 0;
                    pending.push_front(line);
                    continue;
                }
            }
            let (height, overflow) = measure(&line);
            if !overflow && used + gap + height <= capacity {
                used += gap + height;
                page.push(line);
            } else if !page.is_empty() {
                pages.push(std::mem::take(&mut page));
                used = 0;
                pending.push_front(line);
            } else if let Some((left, right)) = split_online_line(&line.text, context) {
                pending.push_front(OnlineLine {
                    text: right,
                    stanza_start: false,
                });
                pending.push_front(OnlineLine {
                    text: left,
                    stanza_start: line.stanza_start,
                });
            } else {
                // One glyph cannot be split further; this only applies to a
                // viewport too small even for a single line below the header.
                pages.push(vec![line]);
            }
        }
        if !page.is_empty() {
            pages.push(page);
        }
        if pages.is_empty() {
            pages.push(Vec::new());
        }
        *self.online_layout.borrow_mut() = Some((context.metrics(), key, pages.clone()));
        pages
    }

    fn online_poem(&self, context: &Context) -> Screen {
        let Some(poem) = self.current_online() else {
            return ScreenBuilder::new("verses-online")
                .top_bar("Verses")
                .splash(
                    Some(Glyph::Search),
                    "Choose a poem",
                    "Open one from Search.",
                )
                .build();
        };
        let pages = self.online_pages(context, poem);
        let page = self.online_page.min(pages.len().saturating_sub(1));
        self.online_page_screen(context, poem, &pages[page], page, pages.len())
    }

    fn paged_poems(
        context: &Context,
        screen: ScreenBuilder,
        rows: &[PoemRow],
        page: usize,
    ) -> (Screen, usize) {
        let labels = rows
            .iter()
            .map(|row| {
                (
                    row.section.as_deref(),
                    row.title.as_str(),
                    row.detail.as_str(),
                )
            })
            .collect::<Vec<_>>();
        let pages = context.paginate_rows_in_sections_under(
            &labels,
            false,
            kobo_sdk::Position::AtTheFoot,
            &screen.clone().build(),
        );
        let count = pages.len().max(1);
        let page = page.min(count - 1);
        let visible = pages.get(page).map(Vec::as_slice).unwrap_or_default();
        let mut screen = screen;
        // Keep rows from each group in one node, so their separators match the
        // SDK's section-aware measurement exactly.
        let mut start = 0;
        while start < visible.len() {
            let first = &rows[visible[start]];
            if let Some(section) = &first.section {
                screen = screen.section(section);
            }
            let end = ((start + 1)..visible.len())
                .find(|&i| rows[visible[i]].section.is_some())
                .unwrap_or(visible.len());
            screen = screen.rows(visible[start..end].iter().map(|&index| {
                let row = &rows[index];
                (
                    row.action.clone(),
                    row.title.clone(),
                    row.detail.clone(),
                    row.glyph,
                )
            }));
            start = end;
        }
        if count > 1 {
            screen = screen
                .page_turns("list-previous", "list-next")
                .page_position(
                    u16::try_from(page + 1).unwrap_or(u16::MAX),
                    u16::try_from(count).unwrap_or(u16::MAX),
                );
        }
        (screen.build(), count)
    }

    fn browse(&self, context: &Context) -> (Screen, usize) {
        let mut screen = ScreenBuilder::new("verses-browse")
            .top_bar("Browse")
            .top_bar_glyph("search", "Search", Glyph::Search);
        if let Some(notice) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, notice);
        }
        let mut rows = self
            .saved
            .favorites
            .iter()
            .filter_map(|id| {
                CORPUS
                    .iter()
                    .position(|poem| poem.id == id.as_str())
                    .map(|index| {
                        let poem = CORPUS[index];
                        PoemRow {
                            section: None,
                            action: format!("poem-{index}"),
                            title: context.clamped_row(poem.title, 2, false),
                            detail: poem.author.into(),
                            glyph: Glyph::Heart,
                        }
                    })
            })
            .collect::<Vec<_>>();
        rows.extend(
            self.saved
                .online_favorites
                .iter()
                .enumerate()
                .map(|(index, poem)| PoemRow {
                    section: None,
                    action: format!("saved-online-{index}"),
                    title: context.clamped_row(&poem.title, 2, false),
                    detail: poem.author.clone(),
                    glyph: Glyph::Heart,
                }),
        );
        if let Some(first) = rows.first_mut() {
            first.section = Some("Favorites".to_owned());
        }
        // By poet, so the shelf reads like the contents of an anthology and
        // a reader after Keats finds his two together. In the order the
        // poems were written down it was seven pages to search by eye.
        let mut by_poet: Vec<usize> = (0..CORPUS.len()).collect();
        by_poet.sort_by_key(|index| (surname(CORPUS[*index].author), CORPUS[*index].year));
        rows.extend(by_poet.iter().enumerate().map(|(place, &index)| {
            let poem = CORPUS[index];
            PoemRow {
                section: (place == 0).then(|| "Poems".to_owned()),
                action: format!("poem-{index}"),
                title: context.clamped_row(poem.title, 2, false),
                detail: format!("{} · {}", poem.author, poem.tags),
                glyph: Glyph::Note,
            }
        }));
        Self::paged_poems(context, screen, &rows, self.browse_page)
    }

    /// What the search box asks for, which follows where the search looks.
    const fn search_prompt(&self) -> &'static str {
        match self.scope {
            Scope::Anything => "A title, a poet, or a line you remember",
            Scope::Title => "A poem's title",
            Scope::Poet => "A poet's name",
            Scope::Line => "Words from a line",
        }
    }

    /// Where a search looks, which poets to start from, and the box that
    /// raises the reader's keyboard.
    ///
    /// The box is a field rather than keys on this screen: the scope and the
    /// suggested poets are taps, and a search that needs neither typing nor
    /// a third of the panel given to keys should not pay for them.
    fn search(&self) -> Screen {
        let mut screen = ScreenBuilder::new("verses-search").top_bar("Search poetry");
        if self.entry.is_open() {
            return screen
                .text_entry(&self.entry, self.search_prompt(), "Search")
                .build();
        }
        if let Some(notice) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, notice.clone());
        }
        let selected = Scope::ALL
            .iter()
            .position(|&(scope, _, _)| scope == self.scope)
            .unwrap_or(0);
        screen = screen
            .segmented(
                selected,
                Scope::ALL.iter().map(|&(_, name, label)| (name, label)),
            )
            .field(SEARCH_FIELD, "", self.search_prompt());
        // Somewhere to start: the whole of PoetryDB is behind this box and an
        // empty one says nothing about it.
        screen
            .section("Poets to start with")
            .chips(
                SUGGESTED_POETS
                    .iter()
                    .map(|poet| (format!("suggest-{poet}"), *poet, false)),
            )
            .build()
    }

    fn results(&self, context: &Context) -> (Screen, usize) {
        let mut screen = ScreenBuilder::new("verses-results")
            .top_bar("Search")
            .top_bar_glyph("search", "New search", Glyph::Search);
        if self.task.is_some() {
            return (
                screen
                    .activity(
                        if matches!(self.pending, Some(Pending::Open(_))) {
                            "Opening poem…"
                        } else {
                            "Searching poetry…"
                        },
                        None,
                    )
                    .build(),
                1,
            );
        }
        if let Some(notice) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, notice);
        }
        if self.results.is_empty() {
            return (
                screen
                    .splash(
                        Some(Glyph::Search),
                        "No poems found",
                        "Try a poet, title, or memorable line.",
                    )
                    .build(),
                1,
            );
        }
        // Grouped by poet, the way an anthology's contents are, so a search
        // for a word that several poets used reads as who used it rather than
        // as a list where the poet is the small print under every title.
        let mut order: Vec<usize> = (0..self.results.len()).collect();
        order.sort_by(|&a, &b| {
            let (a, b) = (&self.results[a], &self.results[b]);
            a.author
                .cmp(&b.author)
                .then_with(|| natural_order(&a.title, &b.title))
        });
        let mut previous: Option<&str> = None;
        let rows = order
            .into_iter()
            .map(|index| {
                let poem = &self.results[index];
                let first = previous != Some(poem.author.as_str());
                previous = Some(poem.author.as_str());
                PoemRow {
                    section: first.then(|| poem.author.clone()),
                    action: format!("result-{index}"),
                    title: context.clamped_row(&poem.title, 2, false),
                    detail: if poem.linecount.trim().is_empty() {
                        String::new()
                    } else {
                        format!(
                            "{} lines · {}",
                            poem.linecount.trim(),
                            reading_time(&poem.linecount)
                        )
                    },
                    glyph: Glyph::Note,
                }
            })
            .collect::<Vec<_>>();
        screen = screen.secondary(format!(
            "{} for \u{201c}{}\u{201d}",
            match self.results.len() {
                1 => "1 poem".to_owned(),
                count => format!("{count} poems"),
            },
            self.query
        ));
        Self::paged_poems(context, screen, &rows, self.results_page)
    }

    fn turn_list(&mut self, context: &Context, forward: bool) {
        let count = match self.view {
            View::Browse => self.browse(context).1,
            View::Results => self.results(context).1,
            _ => return,
        };
        let page = if self.view == View::Browse {
            &mut self.browse_page
        } else {
            &mut self.results_page
        };
        let last = count.saturating_sub(1);
        *page = if forward {
            page.saturating_add(1).min(last)
        } else {
            (*page).min(last).saturating_sub(1)
        };
    }

    fn turn_online(&mut self, context: &Context, forward: bool) {
        let Some(poem) = self.current_online() else {
            return;
        };
        let last = self.online_pages(context, poem).len().saturating_sub(1);
        self.online_page = if forward {
            self.online_page.saturating_add(1).min(last)
        } else {
            self.online_page.min(last).saturating_sub(1)
        };
    }

    fn cancel_request(&mut self, context: &mut Context) {
        if let Some(task) = self.task.take() {
            context.cancel(task);
        }
        self.pending = None;
    }

    fn screen(&self, context: &Context) -> Screen {
        match self.view {
            View::Today if self.daily_poem().is_some() => self.online_poem(context),
            View::Today | View::Reading => self.local_poem(context),
            // The export's own screen: what is being written, where it goes,
            // and a way to try again if the computer is not listening.
            View::Card => self.export.as_ref().map_or_else(
                || self.local_poem(context),
                kobo_sdk::exports::Export::screen,
            ),
            View::Browse => self.browse(context).0,
            View::Search => self.search(),
            View::Results => self.results(context).0,
            View::Online => self.online_poem(context),
        }
    }

    fn save(&mut self, context: &mut Context) {
        match serde_json::to_vec(&self.saved) {
            Ok(bytes) => {
                self.saving = true;
                context.store().save(SETTINGS, bytes);
            }
            Err(_) => {
                self.notice = Some("This favourite could not be written. Try again.".to_owned());
            }
        }
    }

    fn show(&self, context: &mut Context) {
        let screen = self
            .screen(context)
            .with_own_back(!matches!(self.view, View::Today));
        context.set_screen(screen);
    }

    fn begin_search(&mut self, context: &mut Context, query: &str, scope: Scope) {
        if query.trim().is_empty() {
            self.notice = Some("Type something to search for.".into());
            return;
        }
        self.cancel_request(context);
        self.notice = Some("Searching…".into());
        self.results.clear();
        self.online = None;
        self.online_page = 0;
        self.results_page = 0;
        self.view = View::Results;
        query.trim().clone_into(&mut self.query);
        self.task = context.spawn(search_task(query.trim(), scope));
        self.pending = self.task.map(|_| Pending::Search);
        if self.task.is_none() {
            self.notice = Some("Search is busy. Try again in a moment.".into());
        }
    }

    /// The card a reader takes away: the poem, its poet, and where the text
    /// came from, drawn as the panel would draw it.
    ///
    /// A quote with no attribution is the thing the internet is already full
    /// of. Everything here is public domain and says which edition it was
    /// taken from, so a card can be passed on without stripping the poem of
    /// its provenance on the way.
    fn quote_card(&self, context: &Context) -> Screen {
        if let Some(poem) = self.current_online().filter(|_| self.card_from_online()) {
            let pages = self.online_pages(context, poem);
            let page = self.online_page.min(pages.len().saturating_sub(1));
            let screen = ScreenBuilder::new("verses-card")
                .top_bar(context.clamped_row(&poem.title, 1, false))
                .reading(true);
            return set_online_verse(screen, &pages[page])
                .secondary(context.clamped_row(&poem.author, 1, false))
                .secondary("From PoetryDB")
                .build();
        }
        let poem = CORPUS[self.poem];
        let (scale, pages) = self.poem_pages(context);
        let page = self.poem_page.min(pages.len().saturating_sub(1));
        // The card is a picture of the poem, so it is set at the size the poem
        // is set at. It also shares the poem's pagination, and pagination is
        // measured at that size: a card measured at one size and drawn at
        // another breaks its lines where the poem does not.
        let mut screen = ScreenBuilder::new("verses-card")
            .top_bar(poem.title)
            .reading(true)
            .text_scale(scale);
        screen = set_verse(screen, poem, &pages[page]);
        screen
            .secondary(format!("{} · {}", poem.author, poem.year))
            .secondary(poem.source.to_owned())
            .build()
    }

    /// Draws the card and offers it to a paired computer.
    fn export_card(&mut self, context: &mut Context) {
        let metrics = context.metrics();
        let Ok(width) = usize::try_from(metrics.width) else {
            return;
        };
        let Ok(height) = usize::try_from(metrics.height) else {
            return;
        };
        let card = self.quote_card(context);
        let mut surface = kobo_ui::Surface::new(width, height);
        kobo_ui::render(&card, &mut surface, None);
        let picture = kobo_image::encode_png_grey(
            u32::try_from(metrics.width).unwrap_or(0),
            u32::try_from(metrics.height).unwrap_or(0),
            &surface.pixels,
        );
        let name = match self.current_online().filter(|_| self.card_from_online()) {
            Some(poem) => format!("{} by {}", poem.title, poem.author),
            None => format!(
                "{} by {}",
                CORPUS[self.poem].title, CORPUS[self.poem].author
            ),
        };
        match picture
            .map_err(|error| format!("{error:?}"))
            .and_then(|bytes| {
                kobo_sdk::exports::Export::new(&name, kobo_sdk::exports::Format::Png, bytes)
            }) {
            Ok(mut export) => {
                export.begin(context);
                self.export = Some(export);
                self.card_from = self.view;
                self.view = View::Card;
            }
            Err(reason) => {
                context.log(
                    kobo_sdk::LogLevel::Warn,
                    format!("the quote card was refused: {reason}"),
                );
                self.notice = Some("This card could not be prepared.".to_owned());
            }
        }
    }

    /// Moves the Today screen on if the reader's day has changed.
    fn advance_day(&mut self, clock: &dyn Clock) {
        let Some(date) = clock.now().ok().and_then(Snapshot::date) else {
            return;
        };
        let today = (date.year, date.month, date.day);
        if self.day == Some(today) {
            return;
        }
        self.day = Some(today);
        if self.view == View::Today {
            self.poem = daily_index(date.year, date.month, date.day);
            self.poem_page = 0;
        }
    }

    /// Whether the card being made is of a fetched poem rather than a bundled
    /// one: the screen it was asked for from shows one.
    fn card_from_online(&self) -> bool {
        let from = if self.view == View::Card {
            self.card_from
        } else {
            self.view
        };
        from == View::Online || (from == View::Today && self.daily_poem().is_some())
    }

    /// Today's poem from `PoetryDB`, if one has arrived for today.
    fn daily_poem(&self) -> Option<&OnlinePoem> {
        self.saved
            .daily
            .as_ref()
            .filter(|daily| Some(daily.day) == self.day)
            .map(|daily| &daily.poem)
    }

    /// Whether today's poem is on screen, itself or as the card made of it.
    fn showing_today(&self) -> bool {
        self.view == View::Today || (self.view == View::Card && self.card_from == View::Today)
    }

    /// Moves to `view`, starting today's poem at its first page when the
    /// reader comes to it from somewhere else.
    ///
    /// Today's poem and a poem opened from a search turn on the same page
    /// cursor, so arriving at Today from a searched poem's fifth page used to
    /// open today's poem at its fifth page too. Coming back from today's own
    /// card is not arriving from somewhere else, and keeps its page.
    fn arrive_at(&mut self, view: View) {
        if view == View::Today && !self.showing_today() {
            self.online_page = 0;
        }
        self.view = view;
    }

    /// The fetched poem on screen: today's, or one opened from a search.
    fn current_online(&self) -> Option<&OnlinePoem> {
        if self.showing_today() {
            self.daily_poem()
        } else {
            self.online.and_then(|index| self.results.get(index))
        }
    }

    /// Asks for today's poem once the shelf has loaded and the day has none.
    /// Offline it simply fails, and the day keeps its bundled poem.
    fn fetch_daily(&mut self, context: &mut Context) {
        let Some(day) = self.day else { return };
        if !self.loaded || self.daily_task.is_some() || self.daily_poem().is_some() {
            return;
        }
        self.daily_task = context.spawn(daily_task(day));
    }

    fn toggle_favorite(&mut self) {
        if self.view == View::Online || (self.view == View::Today && self.daily_poem().is_some()) {
            let Some(poem) = self.current_online().cloned() else {
                return;
            };
            if let Some(saved) = self
                .saved
                .online_favorites
                .iter()
                .position(|saved| saved.title == poem.title && saved.author == poem.author)
            {
                self.saved.online_favorites.remove(saved);
            } else {
                self.saved.online_favorites.push(poem);
            }
        } else {
            let id = CORPUS[self.poem].id;
            if !self.saved.favorites.remove(id) {
                self.saved.favorites.insert(id.to_owned());
            }
        }
    }
}

impl KoboApp for Verses {
    fn on_start(&mut self, context: &mut Context) {
        context.store().load(SETTINGS);
        self.show(context);
    }

    /// The blob a card is written into answers here rather than in `on_store`,
    /// which is where this first looked for it: the card sat on "saving and
    /// checking" for ever because nothing was listening on this hook.
    fn on_shelf(&mut self, context: &mut Context, name: &str, result: StoreResult) {
        if let Some(export) = self.export.as_mut() {
            if export.on_shelf(context, name, &result) {
                self.show(context);
            }
        }
    }

    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        // A card on its way out answers on its own keys. The settings key is
        // never handed to it: Todo emptied a list once by letting an export
        // claim the key that held the list.
        if let Some(export) = self.export.as_mut() {
            let key = match &result {
                StoreResult::Loaded { key, .. } | StoreResult::Saved { key } => key.clone(),
                _ => String::new(),
            };
            if key != SETTINGS && export.on_save(context, &key, &result) {
                self.show(context);
                return;
            }
        }
        // Storage answers every write, and a refusal is the one answer that
        // has to reach the reader: the favourite is not kept.
        match &result {
            StoreResult::Saved { .. } => {
                self.saving = false;
                if self
                    .notice
                    .as_deref()
                    .is_some_and(|notice| notice.contains("favourite"))
                {
                    self.notice = None;
                }
            }
            StoreResult::Denied(_) => {
                self.saving = false;
                self.notice = Some(
                    "This reader would not keep that favourite. Try marking it again.".to_owned(),
                );
            }
            _ => {}
        }
        if let StoreResult::Loaded {
            value: Some(bytes), ..
        } = result
        {
            if let Ok(saved) = serde_json::from_slice::<Saved>(&bytes) {
                self.saved = saved;
            } else if let Ok(legacy) = serde_json::from_slice::<LegacySaved>(&bytes) {
                // Positions in the shelf as it was then: Hope, The Tiger,
                // Ozymandias, in that order.
                const WAS: [&str; 3] = ["dickinson-hope", "blake-tiger", "shelley-ozymandias"];
                self.saved = Saved {
                    favorites: legacy
                        .favorites
                        .iter()
                        .filter_map(|position| WAS.get(*position).map(|id| (*id).to_owned()))
                        .collect(),
                    online_favorites: legacy.online_favorites,
                    sleep: legacy.sleep,
                    daily: None,
                };
            }
        }
        self.loaded = true;
        self.fetch_daily(context);
        self.show(context);
    }

    #[allow(clippy::too_many_lines)]
    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        self.advance_day(reader_clock().as_ref());
        self.fetch_daily(context);
        // The search box first: while it is open the runtime holds the panel,
        // and its answer arrives as an action like any other. Backing out of
        // it leaves the search screen as it was.
        if self.view == View::Search {
            if let Some(typing) = self.entry.handle(action) {
                if let Typing::Submitted(query) = typing {
                    self.begin_search(context, &query, self.scope);
                }
                self.show(context);
                return;
            }
        }
        if self.view == View::Search && action != ActionId::BACK {
            if let Some(&(scope, _, _)) = Scope::ALL
                .iter()
                .find(|(_, name, _)| action == action_id(name))
            {
                self.scope = scope;
            } else if let Some(poet) = SUGGESTED_POETS
                .iter()
                .find(|poet| action == action_id(&format!("suggest-{poet}")))
            {
                self.begin_search(context, poet, Scope::Poet);
            }
            self.show(context);
            return;
        }

        if action == ActionId::BACK {
            self.cancel_request(context);
            self.notice = None;
            let back = match self.view {
                View::Online => self.online_from,
                View::Reading | View::Results | View::Search => View::Browse,
                View::Card => {
                    self.export = None;
                    self.card_from
                }
                _ => View::Today,
            };
            self.arrive_at(back);
        } else if action == action_id("today") {
            self.cancel_request(context);
            self.arrive_at(View::Today);
        } else if action == action_id("browse") {
            self.cancel_request(context);
            self.view = View::Browse;
        } else if action == action_id("search") {
            self.cancel_request(context);
            self.entry.close();
            self.notice = None;
            self.view = View::Search;
        } else if action == action_id("favorite") {
            self.toggle_favorite();
            self.save(context);
        } else if action == action_id("more-by-author") {
            if let Some(index) = self.online {
                let author = self.results[index].author.clone();
                self.begin_search(context, &author, Scope::Poet);
            }
        } else if action == action_id("card")
            && matches!(self.view, View::Today | View::Reading | View::Online)
        {
            self.export_card(context);
        } else if action == action_id("export-confirm") || action == action_id("export-retry") {
            if let Some(export) = self.export.as_mut() {
                export.begin(context);
            }
        } else if action == action_id("list-next") {
            self.turn_list(context, true);
        } else if action == action_id("list-previous") {
            self.turn_list(context, false);
        } else if action == action_id("online-next") {
            self.turn_online(context, true);
        } else if action == action_id("online-previous") {
            self.turn_online(context, false);
        } else if action == action_id("poem-next") {
            let last = self.poem_pages(context).1.len().saturating_sub(1);
            self.poem_page = (self.poem_page + 1).min(last);
        } else if action == action_id("poem-previous") {
            self.poem_page = self.poem_page.saturating_sub(1);
        } else if let Some(index) =
            (0..CORPUS.len()).find(|index| action == action_id(&format!("poem-{index}")))
        {
            self.poem = index;
            self.poem_page = 0;
            self.view = View::Reading;
        } else if let Some(index) =
            (0..self.results.len()).find(|index| action == action_id(&format!("result-{index}")))
        {
            if self.task.is_some() {
                return;
            }
            self.online_page = 0;
            self.online_from = View::Results;
            if self.results[index].lines.is_empty() {
                self.notice = Some("Opening poem…".into());
                self.task = context.spawn(poem_task(&self.results[index].title));
                self.pending = self.task.map(|_| Pending::Open(index));
                if self.task.is_none() {
                    self.notice = Some("This poem is busy. Try again in a moment.".into());
                }
            } else {
                self.online = Some(index);
                self.view = View::Online;
            }
        } else if let Some(index) = (0..self.saved.online_favorites.len())
            .find(|index| action == action_id(&format!("saved-online-{index}")))
        {
            self.cancel_request(context);
            self.results = vec![self.saved.online_favorites[index].clone()];
            self.online = Some(0);
            self.online_page = 0;
            self.online_from = View::Browse;
            self.view = View::Online;
        }
        self.show(context);
    }

    fn on_page_turn(&mut self, context: &mut Context, forward: bool) {
        match self.view {
            View::Browse | View::Results => self.turn_list(context, forward),
            View::Online => self.turn_online(context, forward),
            View::Today if self.daily_poem().is_some() => self.turn_online(context, forward),
            View::Today | View::Reading => {
                let last = self.poem_pages(context).1.len().saturating_sub(1);
                self.poem_page = if forward {
                    self.poem_page.saturating_add(1).min(last)
                } else {
                    self.poem_page.saturating_sub(1)
                };
            }
            _ => return,
        }
        self.show(context);
    }

    fn on_task(&mut self, context: &mut Context, id: TaskId, outcome: TaskOutcome) {
        if self.daily_task == Some(id) {
            self.daily_task = None;
            let fetched = match outcome {
                TaskOutcome::Completed(bytes) => serde_json::from_slice::<Vec<OnlinePoem>>(&bytes)
                    .ok()
                    .and_then(|poems| {
                        poems.into_iter().find(|poem| {
                            !poem.lines.is_empty() && poem.lines.len() <= MAX_DAILY_LINES
                        })
                    }),
                _ => None,
            };
            if let (Some(poem), Some(day)) = (fetched, self.day) {
                self.saved.daily = Some(DailyPoem { day, poem });
                // The cursor is shared with a poem opened from a search, and
                // this answer arrives whenever the network gets round to it.
                // Starting today's poem at its first page is right only when
                // today's poem is what the reader is looking at; anywhere
                // else it would throw a searched poem back to page one under
                // the reader's thumb.
                if self.showing_today() {
                    self.online_page = 0;
                }
                self.save(context);
                self.show(context);
            }
            return;
        }
        if self.task != Some(id) {
            return;
        }
        self.task = None;
        let pending = self.pending.take();
        match outcome {
            TaskOutcome::Completed(bytes) => match pending {
                Some(Pending::Search) => {
                    if let Ok(results) = serde_json::from_slice::<Vec<OnlinePoem>>(&bytes) {
                        self.results = results;
                        self.notice = None;
                        self.online = None;
                        self.view = View::Results;
                    } else {
                        self.notice = Some("Poetry search couldn't open these results.".into());
                        self.view = View::Results;
                    }
                }
                Some(Pending::Open(index)) => {
                    if let Ok(mut poems) = serde_json::from_slice::<Vec<OnlinePoem>>(&bytes) {
                        if let Some(poem) = poems
                            .drain(..)
                            .find(|poem| poem.author == self.results[index].author)
                        {
                            self.results[index] = poem;
                            self.online = Some(index);
                            self.notice = None;
                            self.view = View::Online;
                        } else {
                            self.notice = Some("That poem isn't available right now.".into());
                        }
                    } else {
                        self.notice = Some("That poem couldn't be opened.".into());
                    }
                }
                None => {}
            },
            // A 404 from this service is its way of saying nothing matched, so
            // it is the one failure that is really an empty result.
            TaskOutcome::Failed(TaskError::NotFound) => {
                if matches!(pending, Some(Pending::Open(_))) {
                    self.notice = Some("That poem isn't available right now.".into());
                    self.view = View::Results;
                } else {
                    self.notice = None;
                    self.results.clear();
                    self.view = View::Results;
                }
            }
            // Anything else is the service, not the search. This used to land
            // with the empty result above, so a poetry service that was down
            // told readers there were no poems matching what they asked for.
            TaskOutcome::Failed(error) => {
                if matches!(pending, Some(Pending::Open(_))) {
                    self.notice = Some("That poem couldn't be opened.".into());
                    self.view = View::Results;
                } else {
                    self.notice = Some(
                        match error {
                            TaskError::Offline => {
                                "This reader is offline. Your shelf is still here."
                            }
                            TaskError::Unauthorized | TaskError::NoCredential => {
                                "The poetry service would not answer this reader."
                            }
                            TaskError::RateLimited(_) => {
                                "The poetry service asked us to wait. Your shelf is still here."
                            }
                            _ => "The poetry service is not answering. Your shelf is still here.",
                        }
                        .to_owned(),
                    );
                    self.view = View::Browse;
                }
            }
            TaskOutcome::Cancelled => {
                self.notice = None;
            }
        }
        self.show(context);
    }
}

/// Split an exceptional line at the renderer's own line/grapheme boundaries.
/// Sets fetched lines as stanzas, the way [`set_verse`] sets a bundled poem.
///
/// Lines of one stanza go together as one paragraph at verse leading. A page
/// was measured a line at a time with paragraph spacing between, so it always
/// has the room this takes.
fn set_online_verse(mut screen: ScreenBuilder, lines: &[OnlineLine]) -> ScreenBuilder {
    let mut stanza: Vec<&str> = Vec::new();
    let mut opens = false;
    let flush = |screen: ScreenBuilder, stanza: &mut Vec<&str>, opens: bool| {
        if stanza.is_empty() {
            return screen;
        }
        let text = stanza.join("\n");
        stanza.clear();
        screen.rich_text(
            text,
            Vec::new(),
            kobo_sdk::ParagraphPresentation {
                margin_before_em: if opens { STANZA_AIR } else { 0 },
                line_height_percent: VERSE_LEADING,
                ..kobo_sdk::ParagraphPresentation::default()
            },
        )
    };
    for line in lines {
        if line.stanza_start {
            screen = flush(screen, &mut stanza, opens);
            opens = true;
        }
        stanza.push(&line.text);
    }
    flush(screen, &mut stanza, opens)
}

/// Sets the runs of a poem as stanzas: one paragraph each, a printed poem's
/// leading inside it and the poet's space between them. The page and the card
/// share this so a card is the page it was made from, line for line.
fn set_verse(mut screen: ScreenBuilder, poem: Poem, runs: &[VerseRun]) -> ScreenBuilder {
    for (index, run) in runs.iter().enumerate() {
        let lines = &poem.stanzas[run.stanza][run.from..run.to];
        screen = screen.rich_text(
            lines.join("\n"),
            Vec::new(),
            kobo_sdk::ParagraphPresentation {
                // A stanza carried over from the page before opens without
                // the space. The unit is hundredths of an em.
                margin_before_em: if index > 0 && run.from == 0 {
                    STANZA_AIR
                } else {
                    0
                },
                // A printed poem's leading rather than a novel's. The reading
                // face opens lines up for an hour of prose, which set a short
                // stanza almost double spaced and pushed the last stanza of
                // Hope onto a page of its own.
                line_height_percent: VERSE_LEADING,
                ..kobo_sdk::ParagraphPresentation::default()
            },
        );
    }
    screen
}

/// How tall one line of a fetched poem stands, alone, and whether it fits the
/// panel at all.
fn measure_online_line(
    line: &OnlineLine,
    metrics: &kobo_sdk::DisplayMetrics,
    chrome: &kobo_ui::Chrome,
) -> (i32, bool) {
    let screen = ScreenBuilder::new("verses-line-measure")
        .reading(true)
        .rich_text(
            line.text.clone(),
            Vec::new(),
            kobo_sdk::ParagraphPresentation {
                margin_before_em: if line.stanza_start { STANZA_AIR } else { 0 },
                ..kobo_sdk::ParagraphPresentation::default()
            },
        )
        .build();
    let measured = screen.diagnostics(metrics, chrome);
    let overflow = measured.issues.iter().any(|issue| {
        matches!(
            issue.kind,
            kobo_ui::LayoutIssueKind::TextOverflow
                | kobo_ui::LayoutIssueKind::Clipped
                | kobo_ui::LayoutIssueKind::ContentOverflow { .. }
                | kobo_ui::LayoutIssueKind::InteractiveOffscreen
        )
    });
    (measured.layout.content_used(), overflow)
}

fn split_online_line(text: &str, context: &Context) -> Option<(String, String)> {
    let metrics = context.metrics();
    let lines = kobo_ui::with_text_scale(metrics.text_scale, || {
        kobo_ui::with_reading_scale(metrics.text_scale, || {
            kobo_ui::wrap_text_in(
                text,
                metrics.readable_width(),
                kobo_ui::FontSize::Body,
                kobo_ui::Face::Reading,
            )
        })
    });
    if lines.len() < 2 {
        return None;
    }
    let mut split = 0;
    for line in lines.iter().take(lines.len() / 2) {
        let start = text[split..].find(line)?;
        split += start + line.len();
    }
    (split > 0 && split < text.len()).then(|| {
        (
            text[..split].trim_end().to_owned(),
            text[split..].trim_start().to_owned(),
        )
    })
}

fn main() -> ExitCode {
    kobo_sdk::run("verses", Verses::default()).map_or_else(
        |error| {
            eprintln!("verses: {error}");
            ExitCode::FAILURE
        },
        |()| ExitCode::SUCCESS,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobo_sdk::clock::ManualClock;
    use kobo_sdk::AppRunner;
    use kobo_ui::{Chrome, CLARA_BW_METRICS};

    #[test]
    fn daily_choice_is_deterministic_and_leap_day_safe() {
        assert_eq!(daily_index(2028, 2, 29), daily_index(2028, 2, 29));
        assert_ne!(daily_index(2026, 9, 1), CORPUS.len());
    }

    /// A clock stopped at a given civil day, at UTC.
    fn clock_on(year: u16, month: u8, day: u8) -> ManualClock {
        // Days from 1970 to the date asked for, the long way round, because a
        // test that computes the answer with the code under test proves
        // nothing.
        let mut days: u64 = 0;
        for past in 1970..year {
            days += if past % 4 == 0 && (past % 100 != 0 || past % 400 == 0) {
                366
            } else {
                365
            };
        }
        let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
        let lengths = [
            31,
            if leap { 29 } else { 28 },
            31,
            30,
            31,
            30,
            31,
            31,
            30,
            31,
            30,
            31,
        ];
        days += lengths.iter().take(usize::from(month - 1)).sum::<u64>();
        days += u64::from(day) - 1;
        ManualClock::new(Snapshot {
            unix_millis: days * 86_400_000 + 12 * 3_600_000,
            monotonic_millis: 0,
            utc_offset_minutes: 0,
        })
        .expect("a valid clock")
    }

    #[test]
    fn the_daily_poem_is_the_readers_day_and_moves_at_midnight() {
        // The day was written into the default state as the first of September
        // 2026, so an application called daily poetry offered one poem for
        // ever.
        let clock = clock_on(2026, 9, 1);
        assert_eq!(poem_for_today(&clock), daily_index(2026, 9, 1));
        let later = clock_on(2027, 3, 14);
        assert_eq!(poem_for_today(&later), daily_index(2027, 3, 14));

        // Crossing midnight with the application open moves the Today screen
        // on rather than leaving yesterday's poem there.
        let mut app = Verses {
            view: View::Today,
            poem: daily_index(2026, 9, 1),
            day: Some((2026, 9, 1)),
            ..Verses::default()
        };
        let midnight = clock_on(2026, 9, 2);
        app.advance_day(&midnight);
        assert_eq!(app.day, Some((2026, 9, 2)));
        assert_eq!(app.poem, daily_index(2026, 9, 2));

        // A poem the reader opened themselves is not replaced under them.
        let mut reading = Verses {
            view: View::Reading,
            poem: 1,
            day: Some((2026, 9, 1)),
            ..Verses::default()
        };
        reading.advance_day(&midnight);
        assert_eq!(reading.poem, 1, "the poem being read was replaced");
        assert_eq!(reading.day, Some((2026, 9, 2)));
    }

    #[test]
    fn every_poem_is_whole_and_says_where_it_came_from() {
        // Each of these used to be four lines: one stanza of The Tiger, a
        // third of Hope, offered with nothing to say it was an excerpt.
        for poem in CORPUS {
            assert!(!poem.author.is_empty() && !poem.source.is_empty() && poem.year < 1929);
            assert!(poem.lines().all(|line| !line.trim().is_empty()));
            assert!(
                poem.stanzas.iter().all(|stanza| !stanza.is_empty()),
                "{}: an empty stanza",
                poem.title
            );
            // Not a proof of completeness, which no test can give: a floor
            // that the four line excerpts this corpus used to carry could not
            // have cleared. The shortest whole poem here is The Eagle, at six.
            assert!(
                poem.line_count() >= 6,
                "{} is {} lines, which is an excerpt rather than a poem",
                poem.title,
                poem.line_count()
            );
        }
    }

    #[test]
    fn a_long_poem_pages_by_stanza_and_keeps_every_line() {
        // A stanza is never split across a page turn, and paging never loses
        // or repeats one.
        let metrics = kobo_ui::DisplayMetrics {
            text_scale: kobo_ui::TextScale::Largest,
            ..CLARA_BW_METRICS
        };
        for (index, poem) in CORPUS.iter().enumerate() {
            for metrics in [CLARA_BW_METRICS, metrics] {
                let context = AppRunner::with_metrics(Verses::default(), metrics).context();
                let app = Verses {
                    poem: index,
                    ..Verses::default()
                };
                let (_, pages) = app.poem_pages(&context);
                let seen: Vec<(usize, usize)> = pages
                    .iter()
                    .flatten()
                    .flat_map(|run| (run.from..run.to).map(move |line| (run.stanza, line)))
                    .collect();
                let every: Vec<(usize, usize)> = poem
                    .stanzas
                    .iter()
                    .enumerate()
                    .flat_map(|(stanza, lines)| (0..lines.len()).map(move |line| (stanza, line)))
                    .collect();
                assert_eq!(
                    seen,
                    every,
                    "{}: lines lost or repeated across {} pages",
                    poem.title,
                    pages.len()
                );
            }
        }
    }

    #[test]
    fn every_page_of_every_poem_fits_at_every_text_size() {
        let chrome = Chrome::measuring(true);
        for scale in kobo_ui::TextScale::STEPS {
            let metrics = kobo_ui::DisplayMetrics {
                text_scale: scale,
                ..CLARA_BW_METRICS
            };
            for (index, poem) in CORPUS.iter().enumerate() {
                let context = AppRunner::with_metrics(Verses::default(), metrics).context();
                let mut app = Verses {
                    poem: index,
                    view: View::Reading,
                    ..Verses::default()
                };
                for page in 0..app.poem_pages(&context).1.len() {
                    app.poem_page = page;
                    let issues = app
                        .local_poem(&context)
                        .diagnostics(&metrics, &chrome)
                        .issues;
                    assert!(
                        issues.is_empty(),
                        "{:?} {} page {page}: {issues:?}",
                        scale,
                        poem.title
                    );
                }
            }
        }
    }

    fn pages_at(index: usize, scale: kobo_ui::TextScale) -> Vec<Vec<VerseRun>> {
        let metrics = kobo_ui::DisplayMetrics {
            text_scale: scale,
            ..CLARA_BW_METRICS
        };
        let context = AppRunner::with_metrics(Verses::default(), metrics).context();
        let app = Verses {
            poem: index,
            view: View::Reading,
            ..Verses::default()
        };
        app.poem_pages(&context).1
    }

    #[test]
    fn a_sonnet_is_one_page_rather_than_eleven_lines_and_then_three() {
        for (index, poem) in CORPUS.iter().enumerate() {
            if poem.stanzas.len() == 1 && poem.line_count() == 14 {
                assert_eq!(
                    pages_at(index, kobo_ui::TextScale::default()).len(),
                    1,
                    "{} is split at the default size",
                    poem.title
                );
            }
        }
    }

    #[test]
    fn a_sonnet_too_tall_for_one_page_is_shared_out_rather_than_left_a_couplet() {
        let sonnet = CORPUS
            .iter()
            .position(|poem| poem.id == "shakespeare-sonnet-18")
            .expect("a sonnet");
        let pages = pages_at(sonnet, kobo_ui::TextScale::Largest);
        assert!(pages.len() > 1);
        let sizes: Vec<usize> = pages
            .iter()
            .map(|runs| runs.iter().map(|run| run.to - run.from).sum())
            .collect();
        let (least, most) = (
            sizes.iter().min().copied().unwrap_or(0),
            sizes.iter().max().copied().unwrap_or(0),
        );
        assert!(most - least <= 1, "uneven pages: {sizes:?}");
    }

    #[test]
    fn a_short_poem_is_one_page_and_a_longer_one_never_breaks_a_stanza() {
        for (index, poem) in CORPUS.iter().enumerate() {
            let pages = pages_at(index, kobo_ui::TextScale::default());
            if poem.line_count() <= 14 {
                assert_eq!(pages.len(), 1, "{} is split", poem.title);
            }
            for run in pages.iter().flatten() {
                assert!(
                    run.from == 0 && run.to == poem.stanzas[run.stanza].len(),
                    "{} breaks a stanza at the default size",
                    poem.title
                );
            }
        }
    }

    #[test]
    fn the_offline_shelf_has_a_month_of_different_poems() {
        assert!(CORPUS.len() >= 30, "only {} poems offline", CORPUS.len());
        let ids: BTreeSet<&str> = CORPUS.iter().map(|poem| poem.id).collect();
        assert_eq!(ids.len(), CORPUS.len(), "two poems share a name");
        let month: BTreeSet<usize> = (1..=30).map(|day| daily_index(2026, 11, day)).collect();
        assert!(month.len() >= 25, "a month offline repeats itself");
        for poem in CORPUS {
            for line in poem.lines() {
                assert!(
                    !line.contains('\u{2014}') && !line.contains('\u{2013}'),
                    "{}: a dash the panel sets badly",
                    poem.title
                );
            }
        }
    }

    #[test]
    fn a_short_poem_from_poetrydb_is_one_page_and_a_long_one_breaks_between_stanzas() {
        let context = AppRunner::new(Verses::default()).context();
        let app = Verses::default();
        let sonnet = OnlinePoem {
            title: "Sonnet".into(),
            author: "A Poet".into(),
            lines: (1..=14)
                .map(|line| format!("The line numbered {line} of the poem"))
                .collect(),
            linecount: "14".into(),
        };
        assert_eq!(app.online_pages(&context, &sonnet).len(), 1);
        let mut lines = Vec::new();
        for stanza in 1..=8 {
            if stanza > 1 {
                lines.push(String::new());
            }
            for line in 1..=4 {
                lines.push(format!("Stanza {stanza}, line {line}, of a long poem"));
            }
        }
        let long = OnlinePoem {
            title: "Long".into(),
            author: "A Poet".into(),
            lines,
            linecount: "32".into(),
        };
        let pages = app.online_pages(&context, &long);
        assert!(pages.len() > 1);
        for page in &pages {
            let first = page.first().expect("a page with lines");
            assert!(
                first.text.ends_with("line 1, of a long poem"),
                "a stanza was broken: {}",
                first.text
            );
        }
    }

    #[test]
    fn poem_actions_are_icons_in_the_header() {
        let context = AppRunner::new(Verses::default()).context();
        let screen = Verses::default().screen(&context);
        let debug = format!("{screen:?}");
        assert!(debug.contains("Heart"), "{debug}");
        assert!(debug.contains("Grid"), "{debug}");
        assert!(!debug.contains("Save favorite"), "{debug}");
    }

    #[test]
    fn poetrydb_searches_titles_authors_and_lines() {
        let Task::Fetch { url, .. } = search_task("hope & spring", Scope::Anything) else {
            panic!("search must use the network");
        };
        assert_eq!(
            url,
            "https://poetrydb.org/author,title,lines/hope%20%26%20spring/author,title,linecount"
        );
    }

    #[test]
    fn every_card_of_every_poem_fits_at_every_text_size() {
        // The card shares the poem's pagination, so it has to share the size
        // that pagination was measured at. It did not: the poem gained a size
        // override and the card kept drawing at the reader's own, so pages
        // measured for one were set in the other and the card broke its lines
        // where the poem did not. Only the poem had a test like this, which is
        // why nothing said so.
        let chrome = Chrome::measuring(true);
        for scale in kobo_ui::TextScale::STEPS {
            let metrics = kobo_ui::DisplayMetrics {
                text_scale: scale,
                ..CLARA_BW_METRICS
            };
            for (index, poem) in CORPUS.iter().enumerate() {
                let context = AppRunner::with_metrics(Verses::default(), metrics).context();
                let mut app = Verses {
                    poem: index,
                    view: View::Reading,
                    ..Verses::default()
                };
                for page in 0..app.poem_pages(&context).1.len() {
                    app.poem_page = page;
                    let card = app.quote_card(&context);
                    let issues = card.diagnostics(&metrics, &chrome).issues;
                    assert!(
                        issues.is_empty(),
                        "{:?} {} card page {page}: {issues:?}",
                        scale,
                        poem.title
                    );
                    // The size it is set at, not merely that it fits. A card
                    // measured large and drawn small still fits: it under-fills
                    // and breaks its lines where the poem does not, which no
                    // overflow diagnostic reports.
                    assert_eq!(
                        card.text_scale,
                        app.local_poem(&context).text_scale,
                        "{:?} {} card page {page} is set at another size than the poem it copies",
                        scale,
                        poem.title
                    );
                }
            }
        }
    }

    #[test]
    fn a_quote_card_is_a_picture_of_the_poem_with_its_poet_on_it() {
        // A quote with no attribution is what the internet is already full of.
        let runner = AppRunner::new(Verses::default());
        let context = runner.context();
        let mut app = Verses {
            view: View::Reading,
            poem: CORPUS
                .iter()
                .position(|poem| poem.id == "shelley-ozymandias")
                .expect("a poem"),
            ..Verses::default()
        };
        let card = app.quote_card(&context);
        let drawn = format!("{card:?}");
        assert!(drawn.contains("Ozymandias"), "no title: {drawn}");
        assert!(drawn.contains("Percy Bysshe Shelley"), "no poet");
        assert!(drawn.contains("1818"), "no year");
        assert!(drawn.contains("Project Gutenberg"), "no source");
        assert!(
            drawn.contains("I met a traveller from an antique land"),
            "no poem"
        );
        assert!(card
            .diagnostics(&CLARA_BW_METRICS, &Chrome::measuring(true))
            .issues
            .is_empty());

        // The card leaves as a picture, which is the thing somebody can pass
        // on, and the export is what carries it to a paired computer.
        let second = AppRunner::new(Verses::default());
        app.export_card(&mut second.context());
        let export = app.export.as_ref().expect("a card on its way out");
        assert_eq!(export.offer().format, kobo_sdk::exports::Format::Png);
        assert!(export.offer().title.contains("Ozymandias"));
        assert_eq!(app.view, View::Card);
    }

    #[test]
    fn a_favourite_survives_the_shelf_being_rewritten() {
        // Favourites were positions in the corpus, so adding a poem or
        // changing the order moved every one of them onto a different poem.
        let mut app = Verses::default();
        let runner = AppRunner::new(Verses::default());
        app.poem = CORPUS
            .iter()
            .position(|poem| poem.id == "shelley-ozymandias")
            .expect("a poem to mark");
        app.toggle_favorite();
        assert!(app.saved.favorites.contains("shelley-ozymandias"));
        assert!(
            !app.saved
                .favorites
                .iter()
                .any(|id| id.parse::<usize>().is_ok()),
            "a favourite is a name rather than a position"
        );
        app.save(&mut runner.context());
        assert!(app.saving, "a write is outstanding until storage answers");
    }

    #[test]
    fn favourites_written_before_poems_had_names_are_still_the_same_poems() {
        let mut app = Verses::default();
        let runner = AppRunner::new(Verses::default());
        // What the old shelf wrote: positions of Hope, The Tiger, Ozymandias.
        let legacy = br#"{"favorites":[0,2],"online_favorites":[],"sleep":false}"#;
        app.on_store(
            &mut runner.context(),
            StoreResult::Loaded {
                key: SETTINGS.to_owned(),
                value: Some(legacy.to_vec()),
            },
        );
        assert!(app.saved.favorites.contains("dickinson-hope"));
        assert!(app.saved.favorites.contains("shelley-ozymandias"));
        assert!(!app.saved.favorites.contains("blake-tiger"));
    }

    #[test]
    fn a_favourite_the_reader_cannot_keep_says_so() {
        let mut app = Verses::default();
        let runner = AppRunner::new(Verses::default());
        app.toggle_favorite();
        app.save(&mut runner.context());
        app.on_store(
            &mut runner.context(),
            StoreResult::Denied(kobo_sdk::StoreError::Unwritable),
        );
        assert!(!app.saving);
        let notice = app.notice.clone().unwrap_or_default();
        assert!(notice.to_lowercase().contains("favourite"), "{notice}");
    }

    // These start on the reader's real day rather than a fixed one. Every tap
    // moves the app on to the clock's day before handling it, so a test pinned
    // to the day it was written passed on that day only, and on every later
    // one asked for a second poem and lost the first.
    #[test]
    fn todays_poem_comes_from_poetrydb_and_stays_for_the_day() {
        let mut runner = AppRunner::new(Verses::default());
        runner.start();
        runner.store_result(StoreResult::Loaded {
            key: SETTINGS.into(),
            value: None,
        });
        let task = runner.app().daily_task.expect("today's poem was asked for");
        let Task::Fetch { url, .. } = daily_task(runner.app().day.expect("the reader's day"))
        else {
            panic!("a fetch");
        };
        assert!(url.contains("linecount,random/"), "{url}");
        let body = br#"[{"title":"A Fetched Poem","author":"A Poet","lines":["One line,","another.","","A second stanza."],"linecount":"3"}]"#;
        runner.task_outcome(task, TaskOutcome::Completed(body.to_vec()));
        let shown = format!("{:?}", runner.app().screen(&runner.context()));
        assert!(shown.contains("A Fetched Poem"), "{shown}");
        assert!(shown.contains("From PoetryDB"));
        assert!(shown.contains("Today's poem"));
        // Kept for the day: a later action asks for nothing more.
        runner.action(action_id("poem-next"));
        assert!(runner.app().daily_task.is_none());
    }

    #[test]
    fn todays_fetched_poem_still_makes_a_quote_card() {
        let mut runner = AppRunner::new(Verses::default());
        runner.start();
        runner.store_result(StoreResult::Loaded {
            key: SETTINGS.into(),
            value: None,
        });
        let task = runner.app().daily_task.expect("asked");
        let body = br#"[{"title":"A Fetched Poem","author":"A Poet","lines":["One line,","another."],"linecount":"2"}]"#;
        runner.task_outcome(task, TaskOutcome::Completed(body.to_vec()));
        assert!(
            format!("{:?}", runner.app().screen(&runner.context())).contains("Make a quote card")
        );
        let card = runner.app().quote_card(&runner.context());
        let drawn = format!("{card:?}");
        assert!(
            drawn.contains("A Fetched Poem") && drawn.contains("A Poet"),
            "{drawn}"
        );
        assert!(drawn.contains("One line,"));
        runner.action(action_id("card"));
        assert_eq!(runner.app().view, View::Card);
        assert!(runner.app().card_from_online());
    }

    /// A reader on the second page of a long poem opened from a search, with
    /// today's poem still on its way.
    fn a_searched_poem_on_its_second_page() -> (AppRunner<Verses>, TaskId) {
        let mut runner = AppRunner::new(Verses::default());
        runner.start();
        runner.store_result(StoreResult::Loaded {
            key: SETTINGS.into(),
            value: None,
        });
        let daily = runner.app().daily_task.expect("today's poem was asked for");
        runner.app_mut().results = vec![OnlinePoem {
            title: "A Long Searched Poem".into(),
            author: "A Poet".into(),
            lines: (1..=200)
                .map(|line| format!("Line {line} of the poem"))
                .collect(),
            linecount: "200".into(),
        }];
        runner.action(action_id("result-0"));
        assert_eq!(runner.app().view, View::Online);
        runner.action(action_id("online-next"));
        assert_eq!(runner.app().online_page, 1, "the poem has a second page");
        (runner, daily)
    }

    const DAILY_BODY: &[u8] = br#"[{"title":"A Fetched Poem","author":"A Poet","lines":["One line,","another."],"linecount":"2"}]"#;

    #[test]
    fn todays_poem_arriving_late_does_not_move_a_searched_poem_back_to_its_first_page() {
        let (mut runner, daily) = a_searched_poem_on_its_second_page();
        runner.task_outcome(daily, TaskOutcome::Completed(DAILY_BODY.to_vec()));
        assert!(runner.app().daily_poem().is_some(), "today's poem was kept");
        assert_eq!(runner.app().view, View::Online);
        assert_eq!(runner.app().online_page, 1);

        // Arriving while the searched poem's card is up, it leaves the card's
        // poem where it was too, so going back lands on the same page.
        let (mut runner, daily) = a_searched_poem_on_its_second_page();
        runner.action(action_id("card"));
        assert_eq!(runner.app().view, View::Card);
        runner.task_outcome(daily, TaskOutcome::Completed(DAILY_BODY.to_vec()));
        runner.action(ActionId::BACK);
        assert_eq!(runner.app().view, View::Online);
        assert_eq!(runner.app().online_page, 1);

        // Today's poem, opened afterwards, starts at its own first page rather
        // than at the page the searched poem was left on.
        runner.action(action_id("today"));
        assert_eq!(runner.app().view, View::Today);
        assert_eq!(runner.app().online_page, 0);
    }

    #[test]
    fn todays_poem_arriving_on_the_today_screen_starts_at_its_first_page() {
        let mut runner = AppRunner::new(Verses::default());
        runner.start();
        runner.store_result(StoreResult::Loaded {
            key: SETTINGS.into(),
            value: None,
        });
        let daily = runner.app().daily_task.expect("today's poem was asked for");
        assert_eq!(runner.app().view, View::Today);
        runner.app_mut().online_page = 3;
        runner.task_outcome(daily, TaskOutcome::Completed(DAILY_BODY.to_vec()));
        assert_eq!(runner.app().online_page, 0);
    }

    #[test]
    fn offline_the_day_keeps_its_bundled_poem() {
        let mut runner = AppRunner::new(Verses::default());
        runner.start();
        runner.store_result(StoreResult::Loaded {
            key: SETTINGS.into(),
            value: None,
        });
        let task = runner.app().daily_task.expect("asked once");
        runner.task_outcome(task, TaskOutcome::Failed(TaskError::Offline));
        let shown = format!("{:?}", runner.app().screen(&runner.context()));
        assert!(shown.contains(CORPUS[runner.app().poem].title));
        assert!(
            runner.app().notice.is_none(),
            "offline is not an error here"
        );
    }

    #[test]
    fn a_numeric_line_count_does_not_empty_the_results() {
        let body = br#"[{"title":"I'm nobody!  Who are you?","author":"Emily Dickinson","linecount":8},{"title":"Ozymandias","author":"Percy Bysshe Shelley","linecount":"14"}]"#;
        let poems: Vec<OnlinePoem> = serde_json::from_slice(body).expect("both forms read");
        assert_eq!(poems[0].linecount, "8");
        assert_eq!(poems[1].linecount, "14");
    }

    #[test]
    fn search_looks_where_the_reader_asked_and_groups_by_poet() {
        for (scope, fields) in [
            (Scope::Anything, "/author,title,lines/"),
            (Scope::Title, "/title/"),
            (Scope::Poet, "/author/"),
            (Scope::Line, "/lines/"),
        ] {
            let Task::Fetch { url, .. } = search_task("sea", scope) else {
                panic!("a fetch");
            };
            assert!(url.contains(fields), "{url}");
        }
        let mut runner = AppRunner::new(Verses::default());
        runner.start();
        runner.action(action_id("browse"));
        runner.action(action_id("search"));
        runner.action(action_id("scope-poet"));
        assert_eq!(runner.app().scope, Scope::Poet);
        assert!(format!("{:?}", runner.app().screen(&runner.context())).contains("Emily Dickinson"));
        runner.action(action_id("suggest-John Keats"));
        assert_eq!(runner.app().view, View::Results);
        let app = runner.app_mut();
        app.task = None;
        app.pending = None;
        app.notice = None;
        app.results = vec![
            OnlinePoem {
                title: "Ode to a Nightingale".into(),
                author: "John Keats".into(),
                lines: Vec::new(),
                linecount: "80".into(),
            },
            OnlinePoem {
                title: "Bright Star".into(),
                author: "John Keats".into(),
                lines: Vec::new(),
                linecount: "14".into(),
            },
        ];
        let shown = format!("{:?}", runner.app().screen(&runner.context()));
        assert!(shown.contains("2 poems for"), "{shown}");
        assert!(shown.contains("14 lines · a minute"));
        assert!(shown.contains("80 lines · four minutes"));
        // Sorted within the poet: Bright Star before the Nightingale.
        assert!(shown.find("Bright Star") < shown.find("Ode to a Nightingale"));
    }

    #[test]
    fn a_service_that_is_down_is_not_reported_as_an_empty_search() {
        // The poetry service answered with its framework's error page for a
        // while, and the application told readers there were no poems matching
        // their search: a server fault dressed as an answer about their words.
        for (error, expected) in [
            (TaskError::Unreachable, "not answering"),
            (TaskError::TimedOut, "not answering"),
            (TaskError::Offline, "offline"),
            (TaskError::Unauthorized, "would not answer"),
        ] {
            let mut app = Verses {
                view: View::Search,
                task: Some(TaskId(1)),
                pending: Some(Pending::Search),
                ..Verses::default()
            };
            let runner = AppRunner::new(Verses::default());
            app.on_task(&mut runner.context(), TaskId(1), TaskOutcome::Failed(error));
            let notice = app.notice.clone().unwrap_or_default();
            assert!(
                notice.to_lowercase().contains(expected),
                "{error:?} became {notice:?}"
            );
            assert!(
                !notice.to_lowercase().contains("no poems"),
                "{error:?} blamed the search"
            );
        }

        // The one failure that really is an empty result: this service answers
        // a search that matched nothing with a 404.
        let mut app = Verses {
            view: View::Search,
            task: Some(TaskId(1)),
            pending: Some(Pending::Search),
            ..Verses::default()
        };
        let runner = AppRunner::new(Verses::default());
        app.on_task(
            &mut runner.context(),
            TaskId(1),
            TaskOutcome::Failed(TaskError::NotFound),
        );
        assert_eq!(app.notice, None);
        assert!(app.results.is_empty());
    }

    #[test]
    fn poetrydb_metadata_results_do_not_need_lines_until_opened() {
        let poems: Vec<OnlinePoem> = serde_json::from_str(
            r#"[{"title":"The Tyger","author":"William Blake","linecount":"24"}]"#,
        )
        .expect("metadata response");
        assert!(poems[0].lines.is_empty());
    }

    #[test]
    fn reading_and_browse_screens_fit() {
        let app = Verses::default();
        let context = AppRunner::new(Verses::default()).context();
        for screen in [
            app.local_poem(&context),
            app.browse(&context).0,
            app.search(),
        ] {
            let diagnostics = screen.diagnostics(&CLARA_BW_METRICS, &Chrome::default());
            assert!(diagnostics.issues.is_empty(), "{:?}", diagnostics.issues);
        }
    }
}

#[cfg(test)]
mod navigation_tests;
