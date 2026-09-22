from pathlib import Path

path = Path("compiler/src/parser.rs")
text = path.read_text()
old = '''        let type_name = if self.matches_simple(&TokenKind::Colon) {
            Some(self.consume_identifier("expected type name after ':'")?)
        } else {
            None
        };
'''
new = '''        let type_name = if self.matches_simple(&TokenKind::Colon) {
            if self.check_identifier_value("maybe") {
                self.advance();
                self.consume_simple(
                    &TokenKind::Live,
                    "expected 'live' after 'maybe' in state-model designation member type",
                )?;
                let model = self.consume_identifier(
                    "expected state model name after 'maybe live' in state-model member",
                )?;
                Some(encode_maybe_live_type_name(&model))
            } else {
                Some(self.consume_identifier("expected type name after ':'")?)
            }
        } else {
            None
        };
'''
count = text.count(old)
if count != 1:
    raise SystemExit(f"expected one state-model type parser anchor, found {count}")
path.write_text(text.replace(old, new, 1))
