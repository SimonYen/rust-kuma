//! 管理员鉴权：密码登录换取内存 token，写操作需携带 token。

use std::collections::HashSet;
use std::sync::Mutex;

pub struct Auth {
    password: String,
    tokens: Mutex<HashSet<String>>,
}

impl Auth {
    pub fn new(password: String) -> Self {
        Auth { password, tokens: Mutex::new(HashSet::new()) }
    }

    /// 校验密码，成功则返回新 token（UUID，内存存储，重启失效）
    pub fn login(&self, password: &str) -> Option<String> {
        if constant_eq(password, &self.password) {
            let token = uuid::Uuid::new_v4().to_string();
            self.tokens.lock().unwrap().insert(token.clone());
            Some(token)
        } else {
            None
        }
    }

    /// 校验 token 是否有效
    pub fn verify(&self, token: &str) -> bool {
        self.tokens.lock().unwrap().contains(token)
    }

    /// 登出：移除 token
    pub fn logout(&self, token: &str) {
        self.tokens.lock().unwrap().remove(token);
    }
}

/// 简单的恒定时间比较，避免时序侧信道
fn constant_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}
