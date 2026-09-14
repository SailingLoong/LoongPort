//! 统一供应商 (Universal Provider) DAO
//!
//! 提供统一供应商的 CRUD 操作。

use crate::database::{to_json_string, Database};
use crate::error::AppError;
use crate::provider::UniversalProvider;
use std::collections::HashMap;

/// 统一供应商的 Settings Key
const UNIVERSAL_PROVIDERS_KEY: &str = "universal_providers";

impl Database {
    /// 获取所有统一供应商
    pub fn get_all_universal_providers(
        &self,
    ) -> Result<HashMap<String, UniversalProvider>, AppError> {
        let result = self.get_setting(UNIVERSAL_PROVIDERS_KEY)?;

        match result {
            Some(json) => serde_json::from_str(&json)
                .map_err(|e| AppError::Database(format!("解析统一供应商数据失败: {e}"))),
            None => Ok(HashMap::new()),
        }
    }

    /// 获取单个统一供应商
    pub fn get_universal_provider(&self, id: &str) -> Result<Option<UniversalProvider>, AppError> {
        let providers = self.get_all_universal_providers()?;
        Ok(providers.get(id).cloned())
    }

    /// 保存统一供应商（添加或更新）
    pub fn save_universal_provider(&self, provider: &UniversalProvider) -> Result<(), AppError> {
        let mut providers = self.get_all_universal_providers()?;
        providers.insert(provider.id.clone(), provider.clone());
        self.save_all_universal_providers(&providers)
    }

    /// 删除统一供应商
    pub fn delete_universal_provider(&self, id: &str) -> Result<bool, AppError> {
        let mut providers = self.get_all_universal_providers()?;
        let existed = providers.remove(id).is_some();
        if existed {
            self.save_all_universal_providers(&providers)?;
        }
        Ok(existed)
    }

    /// 保存所有统一供应商（内部方法）
    fn save_all_universal_providers(
        &self,
        providers: &HashMap<String, UniversalProvider>,
    ) -> Result<(), AppError> {
        let json = to_json_string(providers)?;
        self.set_setting(UNIVERSAL_PROVIDERS_KEY, &json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_universal_provider_uses_protected_settings() {
        let db = Database::memory().unwrap();
        let provider = UniversalProvider::new(
            "universal".into(),
            "Provider".into(),
            "openai".into(),
            "https://api.example".into(),
            "universal-canary".into(),
        );
        db.save_universal_provider(&provider).unwrap();
        assert_eq!(
            db.get_universal_provider(&provider.id)
                .unwrap()
                .unwrap()
                .api_key,
            provider.api_key
        );
        let raw: String = db
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT value FROM settings WHERE key='universal_providers'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!raw.contains("universal-canary"));
        db.conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE settings SET value='corrupt' WHERE key='universal_providers'",
                [],
            )
            .unwrap();
        assert!(db.get_all_universal_providers().is_err());
    }
}
