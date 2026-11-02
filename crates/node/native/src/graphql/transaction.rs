use crate::graphql::api::GraphQLEnum;
use mina_node::rpc::TransactionStatus;

#[derive(Clone, Copy, Debug, GraphQLEnum)]
#[graphql(scalar = crate::graphql::scalars::MinaScalarValue)]
#[allow(non_camel_case_types)]
pub enum GraphQLTransactionStatus {
    INCLUDED,
    PENDING,
    UNKNOWN,
}

impl From<TransactionStatus> for GraphQLTransactionStatus {
    fn from(value: TransactionStatus) -> Self {
        match value {
            TransactionStatus::Included => Self::INCLUDED,
            TransactionStatus::Pending => Self::PENDING,
            TransactionStatus::Unknown => Self::UNKNOWN,
        }
    }
}
