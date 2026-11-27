#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    pub name: &'static str,
    pub monthly_quota: i64,
    pub rate_per_min: i32,
    pub description: &'static str,
}

pub const FREE: Plan = Plan {
    name: "free",
    monthly_quota: 10_000,
    rate_per_min: 60,
    description: "Personal projects and evaluations",
};

pub const PRO: Plan = Plan {
    name: "pro",
    monthly_quota: 1_000_000,
    rate_per_min: 300,
    description: "Production applications",
};

pub const BUSINESS: Plan = Plan {
    name: "business",
    monthly_quota: 10_000_000,
    rate_per_min: 1_000,
    description: "High-volume commercial use",
};

pub const PLANS: [Plan; 3] = [FREE, PRO, BUSINESS];

pub fn find(name: &str) -> Option<Plan> {
    PLANS.iter().copied().find(|plan| plan.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_returns_known_plans() {
        assert_eq!(find("free"), Some(FREE));
        assert_eq!(find("pro"), Some(PRO));
        assert_eq!(find("business"), Some(BUSINESS));
    }

    #[test]
    fn find_rejects_unknown_plan() {
        assert_eq!(find("enterprise"), None);
    }
}
