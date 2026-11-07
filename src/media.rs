//! HP4 media presets. Millimetres in label orientation: `length` runs along the tape,
//! `across` runs across it.

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    /// Continuous tape: the label is as long as its content.
    Tape,
    /// Die-cut labels with gaps the printer detects.
    Gap,
}

#[derive(Debug, Clone, Copy)]
pub struct Media {
    pub name: &'static str,
    pub kind: Kind,
    pub across: f32,
    pub print_across: f32,
    pub length: f32,
    pub print_length: f32,
    pub print_x: f32,
}

const fn tape(name: &'static str, across: f32, print_across: f32) -> Media {
    Media {
        name,
        kind: Kind::Tape,
        across,
        print_across,
        length: 0.0,
        print_length: 0.0,
        print_x: 0.0,
    }
}

const fn label(
    name: &'static str,
    across: f32,
    print_across: f32,
    length: f32,
    print_length: f32,
    print_x: f32,
) -> Media {
    Media {
        name,
        kind: Kind::Gap,
        across,
        print_across,
        length,
        print_length,
        print_x,
    }
}

pub const MEDIA: [Media; 8] = [
    tape("tape15", 15.0, 11.0),
    label("12x40", 12.0, 10.0, 40.0, 35.0, 2.5),
    label("12.5x74", 12.5, 10.5, 74.0, 68.0, 3.0),
    label("15x30", 15.0, 11.0, 30.0, 26.0, 2.0),
    label("14x22", 14.0, 11.0, 22.0, 18.0, 2.0),
    label("14x30", 14.0, 11.0, 30.0, 26.0, 2.0),
    label("14x40", 14.0, 11.0, 40.0, 35.0, 2.5),
    label("14x50", 14.0, 11.0, 50.0, 45.0, 2.5),
];

pub fn find(name: &str) -> Option<Media> {
    MEDIA.iter().copied().find(|m| m.name == name)
}

impl Media {
    /// Printable band across the tape, in dots.
    pub fn across_dots(&self) -> u32 {
        (self.print_across * crate::protocol::DOTS_PER_MM) as u32
    }

    /// Fixed printable length for die-cut labels.
    pub fn length_dots(&self) -> Option<u32> {
        (self.kind == Kind::Gap)
            .then_some((self.print_length * crate::protocol::DOTS_PER_MM) as u32)
    }
}
