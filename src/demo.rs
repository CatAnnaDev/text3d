use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use crate::commands::Command;
use crate::hud::Surface;

pub const FRAME_RATE: f32 = 30.0;
pub const FRAME_STEP: f32 = 1.0 / FRAME_RATE;

const TYPE_LEAD: f32 = 0.12;
const SERVER_DWELL: f32 = 3.0;

#[derive(Clone)]
pub enum Beat {
    AwaitServer(f32),
    Hold(f32),
    Type(&'static str, f32),
    Press(Key, f32),
    Run(Command, f32),
    Aim(Surface, f32),
    Place(usize, usize, f32),
    Orbit(f32, f32, f32),
    Zoom(f32, f32),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Enter,
    Escape,
    Tab,
}

pub enum Action {
    Type(char),
    Press(Key),
    Run(Command),
    Aim(Surface),
    Place(usize, usize),
    Orbit(f32, f32),
    Zoom(f32),
}

pub struct Demo {
    beats: Vec<Beat>,
    index: usize,
    elapsed: f32,
    emitted: usize,
    started: bool,
    waited: f32,
    since_ready: f32,
}

impl Demo {
    pub fn cinema() -> Demo {
        Demo {
            beats: choreography(),
            index: 0,
            elapsed: 0.0,
            emitted: 0,
            started: false,
            waited: 0.0,
            since_ready: 0.0,
        }
    }

    pub fn finished(&self) -> bool {
        self.index >= self.beats.len()
    }

    pub fn holding(&self) -> bool {
        matches!(self.beats.get(self.index), Some(Beat::AwaitServer(_)))
    }

    pub fn advance(&mut self, ready: bool, real_dt: f32, out: &mut Vec<Action>) {
        out.clear();
        let mut budget = 8;
        while budget > 0 {
            budget -= 1;
            let Some(beat) = self.beats.get(self.index).cloned() else {
                return;
            };
            let span = match &beat {
                Beat::AwaitServer(limit) => {
                    self.waited += real_dt;
                    if ready {
                        self.since_ready += real_dt;
                    } else {
                        self.since_ready = 0.0;
                    }
                    if self.since_ready >= SERVER_DWELL || self.waited >= *limit {
                        self.step();
                        continue;
                    }
                    return;
                }
                Beat::Hold(span) => *span,
                Beat::Type(_, span) => *span,
                Beat::Press(_, span) => *span,
                Beat::Run(_, span) => *span,
                Beat::Aim(_, span) => *span,
                Beat::Place(_, _, span) => *span,
                Beat::Orbit(_, _, span) => *span,
                Beat::Zoom(_, span) => *span,
            };

            if !self.started {
                self.started = true;
                match &beat {
                    Beat::Press(key, _) => out.push(Action::Press(*key)),
                    Beat::Run(command, _) => out.push(Action::Run(*command)),
                    Beat::Aim(surface, _) => out.push(Action::Aim(*surface)),
                    Beat::Place(line, column, _) => out.push(Action::Place(*line, *column)),
                    _ => {}
                }
            }

            self.elapsed += FRAME_STEP;

            match &beat {
                Beat::Type(text, span) => {
                    let total = text.chars().count();
                    let lead = (span - TYPE_LEAD).max(0.01);
                    let ratio = (self.elapsed / lead).clamp(0.0, 1.0);
                    let want = (ratio * total as f32).round() as usize;
                    for ch in text.chars().skip(self.emitted).take(want - self.emitted.min(want)) {
                        out.push(Action::Type(ch));
                    }
                    self.emitted = want.max(self.emitted);
                }
                Beat::Orbit(dx, dy, span) => {
                    let share = FRAME_STEP / span.max(FRAME_STEP);
                    out.push(Action::Orbit(dx * share, dy * share));
                }
                Beat::Zoom(amount, span) => {
                    let share = FRAME_STEP / span.max(FRAME_STEP);
                    out.push(Action::Zoom(amount * share));
                }
                _ => {}
            }

            if self.elapsed >= span {
                self.step();
                continue;
            }
            return;
        }
    }

    fn step(&mut self) {
        self.index += 1;
        self.elapsed = 0.0;
        self.emitted = 0;
        self.started = false;
        self.waited = 0.0;
        self.since_ready = 0.0;
    }
}

fn choreography() -> Vec<Beat> {
    vec![
        Beat::AwaitServer(240.0),
        Beat::Hold(2.6),
        Beat::Orbit(150.0, 44.0, 3.6),
        Beat::Hold(1.8),
        Beat::Orbit(-150.0, -44.0, 3.0),
        Beat::Hold(2.0),
        Beat::Place(23, 24, 1.0),
        Beat::Press(Key::Enter, 1.0),
        Beat::Type("        let large = self.", 3.4),
        Beat::Hold(3.0),
        Beat::Type("pl", 0.9),
        Beat::Hold(3.4),
        Beat::Press(Key::Enter, 1.4),
        Beat::Hold(3.0),
        Beat::Place(2, 17, 1.4),
        Beat::Run(Command::GotoDefinition, 3.8),
        Beat::Hold(3.2),
        Beat::Run(Command::QuickOpen, 1.4),
        Beat::Type("extru", 2.2),
        Beat::Hold(3.0),
        Beat::Press(Key::Enter, 1.4),
        Beat::Hold(3.0),
        Beat::Run(Command::Build, 0.6),
        Beat::Orbit(70.0, -18.0, 2.6),
        Beat::Hold(2.4),
        Beat::Orbit(-70.0, 18.0, 2.2),
        Beat::Run(Command::ToggleSidebar, 3.6),
        Beat::Hold(3.4),
        Beat::Aim(Surface::Code, 2.6),
        Beat::Run(Command::ToggleOutput, 3.6),
        Beat::Hold(3.8),
        Beat::Run(Command::Problems, 3.6),
        Beat::Hold(3.4),
        Beat::Aim(Surface::Code, 2.6),
        Beat::Orbit(-60.0, 16.0, 2.4),
        Beat::Hold(2.4),
    ]
}

pub struct Recorder {
    sink: BufWriter<File>,
    path: PathBuf,
    frames: u32,
    width: u32,
    height: u32,
}

impl Recorder {
    pub fn create(path: &Path) -> Result<Recorder, String> {
        let file = File::create(path).map_err(|err| format!("{}: {err}", path.display()))?;
        Ok(Recorder {
            sink: BufWriter::with_capacity(1 << 20, file),
            path: path.to_path_buf(),
            frames: 0,
            width: 0,
            height: 0,
        })
    }

    pub fn push(&mut self, width: u32, height: u32, rgba: &[u8]) -> Result<(), String> {
        if self.frames == 0 {
            self.width = width;
            self.height = height;
        }
        if width != self.width || height != self.height {
            return Err(String::from("taille de frame changeante"));
        }
        self.sink
            .write_all(rgba)
            .map_err(|err| format!("ecriture des frames: {err}"))?;
        self.frames += 1;
        Ok(())
    }

    pub fn frames(&self) -> u32 {
        self.frames
    }

    pub fn finish(mut self) -> Result<String, String> {
        self.sink
            .flush()
            .map_err(|err| format!("fermeture des frames: {err}"))?;
        let notes = self.path.with_extension("txt");
        let summary = format!(
            "{} {} {} {}\n",
            self.width, self.height, self.frames, FRAME_RATE
        );
        std::fs::write(&notes, &summary).map_err(|err| format!("{}: {err}", notes.display()))?;
        Ok(format!(
            "{} frames de {}x{} dans {}",
            self.frames,
            self.width,
            self.height,
            self.path.display()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn la_frappe_sort_caractere_par_caractere_et_en_entier() {
        let mut demo = Demo {
            beats: vec![Beat::Type("abcde", 0.5)],
            index: 0,
            elapsed: 0.0,
            emitted: 0,
            started: false,
            waited: 0.0,
            since_ready: 0.0,
        };
        let mut sortie = Vec::new();
        let mut tape = String::new();
        for _ in 0..40 {
            demo.advance(true, FRAME_STEP, &mut sortie);
            for action in &sortie {
                if let Action::Type(ch) = action {
                    tape.push(*ch);
                }
            }
        }
        assert_eq!(tape, "abcde");
        assert!(demo.finished());
    }

    #[test]
    fn l_attente_du_serveur_ne_consomme_pas_la_suite() {
        let mut demo = Demo {
            beats: vec![Beat::AwaitServer(60.0), Beat::Hold(0.1)],
            index: 0,
            elapsed: 0.0,
            emitted: 0,
            started: false,
            waited: 0.0,
            since_ready: 0.0,
        };
        let mut sortie = Vec::new();
        for _ in 0..600 {
            demo.advance(false, FRAME_STEP, &mut sortie);
            assert!(demo.holding(), "un serveur absent ne doit pas laisser filer la suite");
        }
        demo.advance(true, FRAME_STEP, &mut sortie);
        assert!(
            demo.holding(),
            "la disponibilite seule ne suffit pas, le repos doit etre respecte"
        );
        for _ in 0..((SERVER_DWELL / FRAME_STEP) as usize + 2) {
            demo.advance(true, FRAME_STEP, &mut sortie);
        }
        assert!(!demo.holding(), "apres le repos la suite doit demarrer");
    }

    #[test]
    fn l_attente_du_serveur_abandonne_apres_sa_limite() {
        let mut demo = Demo {
            beats: vec![Beat::AwaitServer(0.05), Beat::Hold(0.05)],
            index: 0,
            elapsed: 0.0,
            emitted: 0,
            started: false,
            waited: 0.0,
            since_ready: 0.0,
        };
        let mut sortie = Vec::new();
        for _ in 0..20 {
            demo.advance(false, FRAME_STEP, &mut sortie);
        }
        assert!(demo.finished());
    }

    #[test]
    fn l_orbite_se_repartit_sur_toute_sa_duree() {
        let mut demo = Demo {
            beats: vec![Beat::Orbit(90.0, 30.0, 1.0)],
            index: 0,
            elapsed: 0.0,
            emitted: 0,
            started: false,
            waited: 0.0,
            since_ready: 0.0,
        };
        let mut sortie = Vec::new();
        let mut somme = (0.0f32, 0.0f32);
        for _ in 0..60 {
            demo.advance(true, FRAME_STEP, &mut sortie);
            for action in &sortie {
                if let Action::Orbit(dx, dy) = action {
                    somme.0 += dx;
                    somme.1 += dy;
                }
            }
        }
        assert!((somme.0 - 90.0).abs() < 1.0, "lacet cumule {}", somme.0);
        assert!((somme.1 - 30.0).abs() < 1.0, "tangage cumule {}", somme.1);
    }
}
