use cssparser::{BasicParseErrorKind, ParseError, Parser, ParserInput, Token};
use lynx_template_decoder::style_info::{ValueToken, token_types};

use crate::ConvertError;

pub(crate) fn value_tokens(source: &str) -> Result<Vec<ValueToken>, ConvertError> {
    let mut input = ParserInput::new(source);
    let mut parser = Parser::new(&mut input);
    let mut tokens = Vec::new();
    tokenize_parser(&mut parser, &mut tokens).map_err(|error| {
        ConvertError::UnsupportedCss(format!(
            "failed to tokenize declaration value {source:?}: {error:?}"
        ))
    })?;
    Ok(tokens)
}

fn tokenize_parser<'i>(
    parser: &mut Parser<'i, '_>,
    output: &mut Vec<ValueToken>,
) -> Result<(), ParseError<'i, ()>> {
    loop {
        let start = parser.position();
        let token = match parser.next_including_whitespace_and_comments() {
            Ok(token) => token.clone(),
            Err(error) if matches!(error.kind, BasicParseErrorKind::EndOfInput) => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        let end = parser.position();
        let raw = parser.slice(start..end);

        match token {
            Token::IncludeMatch
            | Token::DashMatch
            | Token::PrefixMatch
            | Token::SuffixMatch
            | Token::SubstringMatch => {
                for character in raw.chars() {
                    push(output, token_types::DELIM_TOKEN, &character.to_string());
                }
            }
            Token::Function(_) => {
                push(output, token_types::FUNCTION_TOKEN, raw);
                parser.parse_nested_block(|nested| tokenize_parser(nested, output))?;
                push(output, token_types::RIGHT_PARENTHESES_TOKEN, ")");
            }
            Token::ParenthesisBlock => {
                push(output, token_types::LEFT_PARENTHESES_TOKEN, raw);
                parser.parse_nested_block(|nested| tokenize_parser(nested, output))?;
                push(output, token_types::RIGHT_PARENTHESES_TOKEN, ")");
            }
            Token::SquareBracketBlock => {
                push(output, token_types::LEFT_SQUARE_BRACKET_TOKEN, raw);
                parser.parse_nested_block(|nested| tokenize_parser(nested, output))?;
                push(output, token_types::RIGHT_SQUARE_BRACKET_TOKEN, "]");
            }
            Token::CurlyBracketBlock => {
                push(output, token_types::LEFT_CURLY_BRACKET_TOKEN, raw);
                parser.parse_nested_block(|nested| tokenize_parser(nested, output))?;
                push(output, token_types::RIGHT_CURLY_BRACKET_TOKEN, "}");
            }
            other => push(output, token_type(&other), raw),
        }
    }
}

fn push(output: &mut Vec<ValueToken>, token_type: u8, value: &str) {
    output.push(ValueToken {
        token_type,
        value: value.to_owned(),
    });
}

fn token_type(token: &Token<'_>) -> u8 {
    match token {
        Token::Ident(_) => token_types::IDENT_TOKEN,
        Token::AtKeyword(_) => token_types::AT_KEYWORD_TOKEN,
        Token::Hash(_) | Token::IDHash(_) => token_types::HASH_TOKEN,
        Token::QuotedString(_) => token_types::STRING_TOKEN,
        Token::UnquotedUrl(_) => token_types::URL_TOKEN,
        Token::Delim(_)
        | Token::IncludeMatch
        | Token::DashMatch
        | Token::PrefixMatch
        | Token::SuffixMatch
        | Token::SubstringMatch => token_types::DELIM_TOKEN,
        Token::Number { .. } => token_types::NUMBER_TOKEN,
        Token::Percentage { .. } => token_types::PERCENTAGE_TOKEN,
        Token::Dimension { .. } => token_types::DIMENSION_TOKEN,
        Token::WhiteSpace(_) => token_types::WHITESPACE_TOKEN,
        Token::Comment(_) => token_types::COMMENT_TOKEN,
        Token::Colon => token_types::COLON_TOKEN,
        Token::Semicolon => token_types::SEMICOLON_TOKEN,
        Token::Comma => token_types::COMMA_TOKEN,
        Token::CDO => token_types::CDO_TOKEN,
        Token::CDC => token_types::CDC_TOKEN,
        Token::Function(_) => token_types::FUNCTION_TOKEN,
        Token::ParenthesisBlock => token_types::LEFT_PARENTHESES_TOKEN,
        Token::SquareBracketBlock => token_types::LEFT_SQUARE_BRACKET_TOKEN,
        Token::CurlyBracketBlock => token_types::LEFT_CURLY_BRACKET_TOKEN,
        Token::BadUrl(_) => token_types::BAD_URL_TOKEN,
        Token::BadString(_) => token_types::BAD_STRING_TOKEN,
        Token::CloseParenthesis => token_types::RIGHT_PARENTHESES_TOKEN,
        Token::CloseSquareBracket => token_types::RIGHT_SQUARE_BRACKET_TOKEN,
        Token::CloseCurlyBracket => token_types::RIGHT_CURLY_BRACKET_TOKEN,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_text_and_marks_dimensions() {
        let tokens = value_tokens("calc(100rpx - 2px) var(--x, red)").unwrap();
        assert_eq!(
            tokens
                .iter()
                .map(|token| token.value.as_str())
                .collect::<String>(),
            "calc(100rpx - 2px) var(--x, red)"
        );
        assert!(tokens.iter().any(|token| {
            token.token_type == token_types::DIMENSION_TOKEN && token.value == "100rpx"
        }));
    }
}
