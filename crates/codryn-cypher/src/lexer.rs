#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    Match,
    Where,
    Return,
    OrderBy,
    Limit,
    As,
    And,
    Or,
    Not,
    Asc,
    Desc,
    Count,
    Distinct,
    In,
    Contains,
    StartsWith,
    EndsWith,
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Colon,
    Dot,
    Comma,
    Eq,
    Neq,
    Lt,
    Gt,
    Lte,
    Gte,
    Arrow,     // ->
    LeftArrow, // <-
    Dash,      // -
    Star,      // *
    Ident(String),
    StringLit(String),
    IntLit(i64),
    BoolLit(bool),
    Eof,
}

pub struct Lexer<'a> {
    input: &'a str,
    pos: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(input: &'a str) -> Self {
        Self { input, pos: 0 }
    }

    pub fn tokenize(&mut self) -> Vec<Token> {
        let mut tokens = Vec::new();
        loop {
            let tok = self.next_token();
            if tok == Token::Eof {
                tokens.push(tok);
                break;
            }
            tokens.push(tok);
        }
        tokens
    }

    fn next_token(&mut self) -> Token {
        self.skip_whitespace();
        if self.pos >= self.input.len() {
            return Token::Eof;
        }

        let ch = self.peek();
        match ch {
            '(' => {
                self.advance();
                Token::LParen
            }
            ')' => {
                self.advance();
                Token::RParen
            }
            '[' => {
                self.advance();
                Token::LBracket
            }
            ']' => {
                self.advance();
                Token::RBracket
            }
            '{' => {
                self.advance();
                Token::LBrace
            }
            '}' => {
                self.advance();
                Token::RBrace
            }
            ':' => {
                self.advance();
                Token::Colon
            }
            '.' => {
                self.advance();
                Token::Dot
            }
            ',' => {
                self.advance();
                Token::Comma
            }
            '*' => {
                self.advance();
                Token::Star
            }
            '=' => {
                self.advance();
                Token::Eq
            }
            '<' => {
                self.advance();
                if self.peek() == '=' {
                    self.advance();
                    Token::Lte
                } else if self.peek() == '>' {
                    self.advance();
                    Token::Neq
                } else if self.peek() == '-' {
                    self.advance();
                    Token::LeftArrow
                } else {
                    Token::Lt
                }
            }
            '>' => {
                self.advance();
                if self.peek() == '=' {
                    self.advance();
                    Token::Gte
                } else {
                    Token::Gt
                }
            }
            '-' => {
                self.advance();
                if self.peek() == '>' {
                    self.advance();
                    Token::Arrow
                } else {
                    Token::Dash
                }
            }
            '!' => {
                self.advance();
                if self.peek() == '=' {
                    self.advance();
                    Token::Neq
                } else {
                    Token::Not
                }
            }
            '\'' | '"' => self.read_string(),
            c if c.is_ascii_digit() => self.read_number(),
            c if c.is_ascii_alphabetic() || c == '_' => self.read_ident(),
            _ => {
                self.advance();
                self.next_token()
            }
        }
    }

    fn read_string(&mut self) -> Token {
        let quote = self.peek();
        self.advance();
        let start = self.pos;
        while self.pos < self.input.len() && self.peek() != quote {
            self.advance();
        }
        let s = self.input[start..self.pos].to_owned();
        if self.pos < self.input.len() {
            self.advance();
        }
        Token::StringLit(s)
    }

    fn read_number(&mut self) -> Token {
        let start = self.pos;
        while self.pos < self.input.len() && self.peek().is_ascii_digit() {
            self.advance();
        }
        let n: i64 = self.input[start..self.pos].parse().unwrap_or(0);
        Token::IntLit(n)
    }

    fn read_ident(&mut self) -> Token {
        let start = self.pos;
        while self.pos < self.input.len()
            && (self.peek().is_ascii_alphanumeric() || self.peek() == '_')
        {
            self.advance();
        }
        let word = &self.input[start..self.pos];
        match word.to_uppercase().as_str() {
            "MATCH" => Token::Match,
            "WHERE" => Token::Where,
            "RETURN" => Token::Return,
            "ORDER" => {
                self.skip_whitespace();
                if self.pos + 2 <= self.input.len()
                    && self.input[self.pos..].to_uppercase().starts_with("BY")
                {
                    self.pos += 2;
                    Token::OrderBy
                } else {
                    Token::Ident(word.to_owned())
                }
            }
            "LIMIT" => Token::Limit,
            "AS" => Token::As,
            "AND" => Token::And,
            "OR" => Token::Or,
            "NOT" => Token::Not,
            "ASC" => Token::Asc,
            "DESC" => Token::Desc,
            "COUNT" => Token::Count,
            "DISTINCT" => Token::Distinct,
            "IN" => Token::In,
            "CONTAINS" => Token::Contains,
            "STARTS" => {
                self.skip_whitespace();
                if self.input[self.pos..].to_uppercase().starts_with("WITH") {
                    self.pos += 4;
                    Token::StartsWith
                } else {
                    Token::Ident(word.to_owned())
                }
            }
            "ENDS" => {
                self.skip_whitespace();
                if self.input[self.pos..].to_uppercase().starts_with("WITH") {
                    self.pos += 4;
                    Token::EndsWith
                } else {
                    Token::Ident(word.to_owned())
                }
            }
            "TRUE" => Token::BoolLit(true),
            "FALSE" => Token::BoolLit(false),
            _ => Token::Ident(word.to_owned()),
        }
    }

    fn peek(&self) -> char {
        self.input[self.pos..].chars().next().unwrap_or('\0')
    }

    fn advance(&mut self) {
        if self.pos < self.input.len() {
            self.pos += self.peek().len_utf8();
        }
    }

    fn skip_whitespace(&mut self) {
        while self.pos < self.input.len() && self.peek().is_whitespace() {
            self.advance();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_tokenize() {
        let mut lex = Lexer::new("MATCH (n:Function) RETURN n.name LIMIT 10");
        let tokens = lex.tokenize();
        assert_eq!(tokens[0], Token::Match);
        assert_eq!(tokens[1], Token::LParen);
        assert_eq!(tokens[2], Token::Ident("n".into()));
        assert_eq!(tokens[3], Token::Colon);
        assert_eq!(tokens[4], Token::Ident("Function".into()));
        assert_eq!(tokens[5], Token::RParen);
    }
}
