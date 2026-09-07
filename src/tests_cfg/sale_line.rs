use crate as sea_orm;
use sea_orm::entity::prelude::*;

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "sale_line")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub sale_id: i32,
    pub product_id: i32,
    pub quantity: i32,
    pub unit_price: Decimal,
    /// Absolute discount applied per unit (already in money). Nullable.
    pub discount: Option<Decimal>,
    /// VAT rate expressed as a percentage, e.g. `20` means 20%.
    pub vat_rate: Decimal,
    #[sea_orm(belongs_to, from = "sale_id", to = "id")]
    pub sale: BelongsTo<super::sale::Entity>,
    #[sea_orm(belongs_to, from = "product_id", to = "id")]
    pub product: BelongsTo<super::product::Entity>,
}

impl ActiveModelBehavior for ActiveModel {}
