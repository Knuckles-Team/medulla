//! Private operation-family routing for the data-science handler.

use medulla_protocol_types::result_contract::compute as results;
use medulla_result_types::compute_result::datascience::TrainTestSplitResult;

use medulla_protocol_types::protocol::Method;
use medulla_protocol_types::protocol::Response;
use medulla_protocol_types::protocol::ResultPayload;
#[derive(Clone, Copy)]
enum DataScienceRoute {
    Primitives,
    Training,
    Other,
}

fn route_for(method: &Method) -> DataScienceRoute {
    match method {
        Method::DsLinearRegression { .. }
        | Method::DsKMeans { .. }
        | Method::DsPca { .. }
        | Method::DsComputeStats { .. }
        | Method::DsTrainTestSplit { .. }
        | Method::DsFitEstimator { .. }
        | Method::DsPredictEstimator { .. } => DataScienceRoute::Primitives,
        Method::DsSoftmax { .. }
        | Method::DsLogSoftmax { .. }
        | Method::DsCrossEntropy { .. }
        | Method::DsDpoLoss { .. }
        | Method::DsGrpoSurrogate { .. }
        | Method::DsKlDivergence { .. }
        | Method::DsAdamStep { .. }
        | Method::DsSgdStep { .. } => DataScienceRoute::Training,
        _ => DataScienceRoute::Other,
    }
}

pub fn try_handle(req_id: u64, method: Method) -> Result<Response, Method> {
    match route_for(&method) {
        DataScienceRoute::Primitives => handle_primitives(req_id, method),
        DataScienceRoute::Training => handle_training(req_id, method),
        DataScienceRoute::Other => Err(method),
    }
}

fn handle_primitives(req_id: u64, method: Method) -> Result<Response, Method> {
    Ok(match method {
        // ── Data Science Primitives (CONCEPT:MEDULLA-KG.compute.rust-native-training-loss) ─────────────
        Method::DsLinearRegression { x, y } => {
            let result = medulla_compute::datascience::primitives::linear_regression(&x, &y);
            Response::ok(
                req_id,
                ResultPayload::of::<results::DsLinearRegression>(result),
            )
        }
        Method::DsKMeans { data, k, max_iter } => {
            let result = medulla_compute::datascience::primitives::kmeans(&data, k, max_iter);
            Response::ok(req_id, ResultPayload::of::<results::DsKMeans>(result))
        }
        Method::DsPca { data, n_components } => {
            let result = medulla_compute::datascience::primitives::pca(&data, n_components);
            Response::ok(req_id, ResultPayload::of::<results::DsPca>(result))
        }
        Method::DsComputeStats { data } => {
            let result = medulla_compute::datascience::primitives::compute_stats(&data);
            Response::ok(req_id, ResultPayload::of::<results::DsComputeStats>(result))
        }
        Method::DsTrainTestSplit {
            data,
            labels,
            test_ratio,
            shuffle,
            seed,
        } => {
            let (x_train, x_test, y_train, y_test) =
                medulla_compute::datascience::primitives::train_test_split(
                    &data, &labels, test_ratio, shuffle, seed,
                );
            Response::ok(
                req_id,
                ResultPayload::of::<results::DsTrainTestSplit>(TrainTestSplitResult {
                    x_train,
                    x_test,
                    y_train,
                    y_test,
                }),
            )
        }
        Method::DsFitEstimator {
            estimator,
            x,
            y,
            params,
        } => match medulla_compute::datascience::estimators::fit_estimator(&estimator, &x, &y, &params) {
            Ok(model) => Response::ok(req_id, ResultPayload::of::<results::DsFitEstimator>(model)),
            Err(e) => Response::err(req_id, e),
        },
        Method::DsPredictEstimator { model, x } => {
            let preds = medulla_compute::datascience::estimators::predict(&model, &x);
            Response::ok(
                req_id,
                ResultPayload::of::<results::DsPredictEstimator>(preds),
            )
        }
        other => return Err(other),
    })
}

fn handle_training(req_id: u64, method: Method) -> Result<Response, Method> {
    Ok(match method {
        // ── Training loss / optimizer kernels (CONCEPT:MEDULLA-KG.compute.rust-native-training-loss) ──
        Method::DsSoftmax {
            logits,
            temperature,
        } => {
            let r = medulla_compute::datascience::training::softmax(&logits, temperature);
            Response::ok(req_id, ResultPayload::of::<results::DsSoftmax>(r))
        }
        Method::DsLogSoftmax { logits } => {
            let r = medulla_compute::datascience::training::log_softmax(&logits);
            Response::ok(req_id, ResultPayload::of::<results::DsLogSoftmax>(r))
        }
        Method::DsCrossEntropy { logits, labels } => {
            let r = medulla_compute::datascience::training::cross_entropy(&logits, &labels);
            Response::ok(req_id, ResultPayload::of::<results::DsCrossEntropy>(r))
        }
        Method::DsDpoLoss {
            policy_chosen,
            policy_rejected,
            ref_chosen,
            ref_rejected,
            beta,
        } => {
            let r = medulla_compute::datascience::training::dpo_loss(
                &policy_chosen,
                &policy_rejected,
                &ref_chosen,
                &ref_rejected,
                beta,
            );
            Response::ok(req_id, ResultPayload::of::<results::DsDpoLoss>(r))
        }
        Method::DsGrpoSurrogate {
            logprob,
            old_logprob,
            advantage,
            clip_eps,
        } => {
            let r = medulla_compute::datascience::training::grpo_surrogate(
                &logprob,
                &old_logprob,
                &advantage,
                clip_eps,
            );
            Response::ok(req_id, ResultPayload::of::<results::DsGrpoSurrogate>(r))
        }
        Method::DsKlDivergence {
            logprob,
            ref_logprob,
        } => {
            let r = medulla_compute::datascience::training::kl_divergence(&logprob, &ref_logprob);
            Response::ok(req_id, ResultPayload::scalar::<results::DsKlDivergence>(r))
        }
        Method::DsAdamStep {
            params,
            grads,
            m,
            v,
            lr,
            beta1,
            beta2,
            eps,
            t,
        } => {
            let r = medulla_compute::datascience::training::adam_step(
                &params,
                &grads,
                &m,
                &v,
                medulla_compute::datascience::training::AdamHyperparameters {
                    lr,
                    beta1,
                    beta2,
                    eps,
                },
                t,
            );
            Response::ok(req_id, ResultPayload::of::<results::DsAdamStep>(r))
        }
        Method::DsSgdStep { params, grads, lr } => {
            let r = medulla_compute::datascience::training::sgd_step(&params, &grads, lr);
            Response::ok(req_id, ResultPayload::of::<results::DsSgdStep>(r))
        }
        other => return Err(other),
    })
}
