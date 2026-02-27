use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Priority {
    P0,
    P1,
    P2,
}

impl Priority {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::P0 => "P0",
            Self::P1 => "P1",
            Self::P2 => "P2",
        }
    }

    pub fn from_db(value: &str) -> Option<Self> {
        match value {
            "P0" => Some(Self::P0),
            "P1" => Some(Self::P1),
            "P2" => Some(Self::P2),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Priority;

    #[test]
    fn db_round_trip_strings_are_stable() {
        for value in [Priority::P0, Priority::P1, Priority::P2] {
            assert_eq!(Priority::from_db(value.as_str()), Some(value));
        }
        assert_eq!(Priority::from_db("bad"), None);
    }
}
