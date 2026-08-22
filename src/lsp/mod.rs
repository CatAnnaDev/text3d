pub mod protocol;
pub mod session;
pub mod transport;

pub mod position {
    pub fn char_to_utf16(line: &str, column: usize) -> u32 {
        if line.is_ascii() {
            return line.len().min(column) as u32;
        }
        let mut units = 0u32;
        let mut index = 0usize;
        for character in line.chars() {
            if index == column {
                break;
            }
            units += character.len_utf16() as u32;
            index += 1;
        }
        units
    }

    pub fn utf16_to_char(line: &str, unit: u32) -> usize {
        if line.is_ascii() {
            return (unit as usize).min(line.len());
        }
        let mut units = 0u32;
        let mut index = 0usize;
        for character in line.chars() {
            let width = character.len_utf16() as u32;
            if units >= unit || units + width > unit {
                return index;
            }
            units += width;
            index += 1;
        }
        index
    }

    pub fn byte_to_utf16(line: &str, byte: usize) -> u32 {
        if line.is_ascii() {
            return line.len().min(byte) as u32;
        }
        let limit = byte.min(line.len());
        let mut units = 0u32;
        for (offset, character) in line.char_indices() {
            if offset >= limit {
                break;
            }
            units += character.len_utf16() as u32;
        }
        units
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn ascii_identique() {
            let line = "fn principale() {";
            assert_eq!(char_to_utf16(line, 0), 0);
            assert_eq!(char_to_utf16(line, 3), 3);
            assert_eq!(char_to_utf16(line, 999), line.len() as u32);
            assert_eq!(utf16_to_char(line, 3), 3);
            assert_eq!(utf16_to_char(line, 999), line.len());
            assert_eq!(byte_to_utf16(line, 4), 4);
        }

        #[test]
        fn accents_une_unite() {
            let line = "let caf\u{e9} = 1;";
            assert_eq!(line.chars().count(), 13);
            assert_eq!(line.len(), 14);
            assert_eq!(char_to_utf16(line, 13), 13);
            assert_eq!(char_to_utf16(line, 8), 8);
            assert_eq!(utf16_to_char(line, 13), 13);
            assert_eq!(byte_to_utf16(line, 14), 13);
            assert_eq!(byte_to_utf16(line, 7), 7);
        }

        #[test]
        fn cjk_une_unite() {
            let line = "\u{4e2d}\u{6587}abc";
            assert_eq!(char_to_utf16(line, 2), 2);
            assert_eq!(char_to_utf16(line, 5), 5);
            assert_eq!(utf16_to_char(line, 2), 2);
            assert_eq!(byte_to_utf16(line, 6), 2);
        }

        #[test]
        fn emoji_deux_unites() {
            let line = "a\u{1f600}b";
            assert_eq!(line.chars().count(), 3);
            assert_eq!(char_to_utf16(line, 1), 1);
            assert_eq!(char_to_utf16(line, 2), 3);
            assert_eq!(char_to_utf16(line, 3), 4);
            assert_eq!(utf16_to_char(line, 0), 0);
            assert_eq!(utf16_to_char(line, 1), 1);
            assert_eq!(utf16_to_char(line, 2), 1);
            assert_eq!(utf16_to_char(line, 3), 2);
            assert_eq!(utf16_to_char(line, 4), 3);
            assert_eq!(byte_to_utf16(line, line.len()), 4);
        }

        #[test]
        fn aller_retour_sur_ligne_mixte() {
            let line = "x = \"caf\u{e9} \u{1f680} \u{4e2d}\";";
            for column in 0..=line.chars().count() {
                let unit = char_to_utf16(line, column);
                assert_eq!(utf16_to_char(line, unit), column, "colonne {column}");
            }
        }

        #[test]
        fn drapeau_hors_plan_de_base() {
            let line = "\u{1f1eb}\u{1f1f7} fin";
            assert_eq!(char_to_utf16(line, 2), 4);
            assert_eq!(utf16_to_char(line, 4), 2);
            assert_eq!(char_to_utf16(line, line.chars().count()), 8);
        }
    }
}
