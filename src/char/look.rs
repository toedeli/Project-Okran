//! A character's looks as plain data, so the simulation and saves can carry them without an engine.

/// What to build: race record name, head record name (default: the race's first male head), hair
/// and beard attachment names, worn ARMOUR names, and the hair tint.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CharacterSpec {
    pub race: String,
    pub head: Option<String>,
    pub hairs: Vec<String>,
    pub wear: Vec<String>,
    pub hair_colour: [f32; 3],
    /// The female body (the race's "female mesh", heads, skin and ragdoll; clothes' and hairs'
    /// "mesh female").
    pub female: bool,
}

impl CharacterSpec {
    pub fn new(race: &str) -> Self {
        CharacterSpec {
            race: race.into(),
            head: None,
            hairs: Vec::new(),
            wear: Vec::new(),
            hair_colour: [0.24, 0.16, 0.1],
            female: false,
        }
    }
}
