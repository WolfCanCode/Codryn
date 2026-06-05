use crate::lexer::{Lexer, Token};
use crate::*;
use anyhow::{bail, Result};

pub fn parse(input: &str) -> Result<CypherQuery> {
    let mut lex = Lexer::new(input);
    let tokens = lex.tokenize();
    let mut p = Parser { tokens, pos: 0 };
    p.parse_query()
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> &Token {
        self.tokens.get(self.pos).unwrap_or(&Token::Eof)
    }

    fn advance(&mut self) -> Token {
        let tok = self.tokens.get(self.pos).cloned().unwrap_or(Token::Eof);
        self.pos += 1;
        tok
    }

    fn expect(&mut self, expected: &Token) -> Result<()> {
        let tok = self.advance();
        if &tok != expected {
            bail!("expected {:?}, got {:?}", expected, tok);
        }
        Ok(())
    }

    fn parse_query(&mut self) -> Result<CypherQuery> {
        self.expect(&Token::Match)?;
        let match_clause = self.parse_match()?;

        let where_clause = if *self.peek() == Token::Where {
            self.advance();
            Some(self.parse_where()?)
        } else {
            None
        };

        self.expect(&Token::Return)?;
        let return_clause = self.parse_return()?;

        let order_by = if *self.peek() == Token::OrderBy {
            self.advance();
            Some(self.parse_order_by()?)
        } else {
            None
        };

        let limit = if *self.peek() == Token::Limit {
            self.advance();
            match self.advance() {
                Token::IntLit(n) => Some(n),
                _ => bail!("expected integer after LIMIT"),
            }
        } else {
            None
        };

        Ok(CypherQuery {
            match_clause,
            where_clause,
            return_clause,
            order_by,
            limit,
        })
    }

    fn parse_match(&mut self) -> Result<MatchClause> {
        let mut patterns = Vec::new();
        loop {
            let node1 = self.parse_node_pattern()?;

            if *self.peek() == Token::Dash || *self.peek() == Token::LeftArrow {
                let rel = self.parse_rel_pattern()?;
                let node2 = self.parse_node_pattern()?;
                patterns.push(Pattern::Relationship(node1, rel, node2));
            } else {
                patterns.push(Pattern::Node(node1));
            }

            if *self.peek() == Token::Comma {
                self.advance();
            } else {
                break;
            }
        }
        Ok(MatchClause { patterns })
    }

    fn parse_node_pattern(&mut self) -> Result<NodePattern> {
        self.expect(&Token::LParen)?;
        let mut variable = None;
        let mut label = None;

        if let Token::Ident(_) = self.peek() {
            if let Token::Ident(name) = self.advance() {
                variable = Some(name);
            }
        }

        if *self.peek() == Token::Colon {
            self.advance();
            if let Token::Ident(l) = self.advance() {
                label = Some(l);
            }
        }

        self.expect(&Token::RParen)?;
        Ok(NodePattern { variable, label })
    }

    fn parse_rel_pattern(&mut self) -> Result<RelPattern> {
        let mut direction = Direction::Right;

        if *self.peek() == Token::LeftArrow {
            self.advance();
            direction = Direction::Left;
        } else {
            self.expect(&Token::Dash)?;
        }

        let mut variable = None;
        let mut rel_type = None;

        if *self.peek() == Token::LBracket {
            self.advance();
            if let Token::Ident(_) = self.peek() {
                if let Token::Ident(v) = self.advance() {
                    variable = Some(v);
                }
            }
            if *self.peek() == Token::Colon {
                self.advance();
                if let Token::Ident(t) = self.advance() {
                    rel_type = Some(t);
                }
            }
            self.expect(&Token::RBracket)?;
        }

        if *self.peek() == Token::Arrow {
            self.advance();
            if direction == Direction::Left {
                direction = Direction::Both;
            } else {
                direction = Direction::Right;
            }
        } else if *self.peek() == Token::Dash {
            self.advance();
            if direction != Direction::Left {
                direction = Direction::Both;
            }
        }

        Ok(RelPattern {
            variable,
            rel_type,
            direction,
        })
    }

    fn parse_where(&mut self) -> Result<WhereClause> {
        let mut conditions = Vec::new();
        conditions.push(self.parse_condition()?);
        while *self.peek() == Token::And {
            self.advance();
            conditions.push(self.parse_condition()?);
        }
        Ok(WhereClause { conditions })
    }

    fn parse_condition(&mut self) -> Result<Condition> {
        let prop = self.parse_property_ref()?;

        match self.peek().clone() {
            Token::Eq => {
                self.advance();
                let val = self.parse_value()?;
                Ok(Condition::Eq(prop, val))
            }
            Token::Contains => {
                self.advance();
                if let Token::StringLit(s) = self.advance() {
                    Ok(Condition::Contains(prop, s))
                } else {
                    bail!("expected string after CONTAINS")
                }
            }
            Token::StartsWith => {
                self.advance();
                if let Token::StringLit(s) = self.advance() {
                    Ok(Condition::StartsWith(prop, s))
                } else {
                    bail!("expected string after STARTS WITH")
                }
            }
            _ => bail!("unexpected token in WHERE: {:?}", self.peek()),
        }
    }

    fn parse_property_ref(&mut self) -> Result<PropertyRef> {
        let variable = match self.advance() {
            Token::Ident(v) => v,
            t => bail!("expected identifier, got {:?}", t),
        };
        self.expect(&Token::Dot)?;
        let property = match self.advance() {
            Token::Ident(p) => p,
            t => bail!("expected property name, got {:?}", t),
        };
        Ok(PropertyRef { variable, property })
    }

    fn parse_value(&mut self) -> Result<Value> {
        match self.advance() {
            Token::StringLit(s) => Ok(Value::String(s)),
            Token::IntLit(n) => Ok(Value::Int(n)),
            Token::BoolLit(b) => Ok(Value::Bool(b)),
            t => bail!("expected value, got {:?}", t),
        }
    }

    fn parse_return(&mut self) -> Result<ReturnClause> {
        let mut items = Vec::new();
        loop {
            items.push(self.parse_return_item()?);
            if *self.peek() == Token::Comma {
                self.advance();
            } else {
                break;
            }
        }
        Ok(ReturnClause { items })
    }

    fn parse_return_item(&mut self) -> Result<ReturnItem> {
        if *self.peek() == Token::Count {
            self.advance();
            self.expect(&Token::LParen)?;
            let inner = match self.advance() {
                Token::Ident(v) => v,
                Token::Star => "*".to_owned(),
                t => bail!("expected identifier in COUNT, got {:?}", t),
            };
            self.expect(&Token::RParen)?;
            return Ok(ReturnItem::Count(inner));
        }

        let ident = match self.advance() {
            Token::Ident(v) => v,
            t => bail!("expected identifier in RETURN, got {:?}", t),
        };

        if *self.peek() == Token::Dot {
            self.advance();
            let prop = match self.advance() {
                Token::Ident(p) => p,
                t => bail!("expected property, got {:?}", t),
            };
            Ok(ReturnItem::Property(PropertyRef {
                variable: ident,
                property: prop,
            }))
        } else {
            Ok(ReturnItem::Variable(ident))
        }
    }

    fn parse_order_by(&mut self) -> Result<Vec<OrderItem>> {
        let mut items = Vec::new();
        loop {
            let expr = self.parse_return_item()?;
            let descending = if *self.peek() == Token::Desc {
                self.advance();
                true
            } else {
                if *self.peek() == Token::Asc {
                    self.advance();
                }
                false
            };
            items.push(OrderItem { expr, descending });
            if *self.peek() == Token::Comma {
                self.advance();
            } else {
                break;
            }
        }
        Ok(items)
    }
}

impl PartialEq for Direction {
    fn eq(&self, other: &Self) -> bool {
        matches!(
            (self, other),
            (Direction::Right, Direction::Right)
                | (Direction::Left, Direction::Left)
                | (Direction::Both, Direction::Both)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple() {
        let q = parse("MATCH (n:Function) RETURN n.name LIMIT 10").unwrap();
        assert_eq!(q.limit, Some(10));
        assert_eq!(q.return_clause.items.len(), 1);
    }

    #[test]
    fn test_parse_relationship() {
        let q = parse("MATCH (a:Function)-[r:CALLS]->(b:Function) RETURN a.name, b.name").unwrap();
        assert_eq!(q.match_clause.patterns.len(), 1);
        assert!(matches!(
            q.match_clause.patterns[0],
            Pattern::Relationship(..)
        ));
    }

    #[test]
    fn test_parse_where() {
        let q = parse("MATCH (n:Function) WHERE n.name = 'main' RETURN n").unwrap();
        assert!(q.where_clause.is_some());
    }
}
