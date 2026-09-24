//! `convaiinnovations/laya` Native Candle yükleyici: ModernBERT-large + option-marker karar başlığı.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use candle_core::{DType, Device, IndexOp, Result as CandleResult, Tensor, D};
use candle_nn::{
    embedding, layer_norm, linear, ops::softmax, Embedding, LayerNorm, Linear, VarBuilder,
};
use candle_transformers::models::modernbert::{Config as BertConfig, ModernBert};
use serde::Deserialize;

pub const QTYPE_CHOICE: i64 = 0;
pub const QTYPE_SCORE: i64 = 1;
pub const QTYPE_NOUL: i64 = 2;
pub const MAX_LEN: usize = 512;
pub const HEAD_MAX_LEN: usize = 192;
pub const HIDDEN: usize = 1024;

#[derive(Debug, Clone)]
pub struct LayaRuntimeConfig {
    pub max_len: usize,
    pub head_max_len: usize,
    pub temperature: [f32; 3],
    pub temperature_by_options: HashMap<String, f32>,
}

impl Default for LayaRuntimeConfig {
    fn default() -> Self {
        Self {
            max_len: MAX_LEN,
            head_max_len: HEAD_MAX_LEN,
            temperature: [1.636_903, 1.251_43, 1.983_399_5],
            temperature_by_options: default_temp_map(),
        }
    }
}

fn default_temp_map() -> HashMap<String, f32> {
    HashMap::from([
        ("choice:3-5".into(), 1.760_151_9),
        ("choice:6-10".into(), 1.000_015_9),
        ("score:3-5".into(), 1.251_43),
        ("noul:2".into(), 1.983_399_5),
        ("choice:11+".into(), 0.100_582_81),
        ("choice:2".into(), 1.906_356_3),
    ])
}

pub fn temp_bucket(qtype: i64, k: usize) -> String {
    let name = match qtype {
        QTYPE_SCORE => "score",
        QTYPE_NOUL => "noul",
        _ => "choice",
    };
    let size = if k <= 2 {
        "2"
    } else if k <= 5 {
        "3-5"
    } else if k <= 10 {
        "6-10"
    } else {
        "11+"
    };
    format!("{name}:{size}")
}

pub fn temperature_for(cfg: &LayaRuntimeConfig, qtype: i64, k: usize) -> f32 {
    let key = temp_bucket(qtype, k);
    cfg.temperature_by_options
        .get(&key)
        .copied()
        .unwrap_or_else(|| {
            let idx = qtype.clamp(0, 2) as usize;
            cfg.temperature[idx]
        })
        .max(1e-4)
}

pub fn softmax_temp(logits: &[f32], temperature: f32) -> Vec<f32> {
    if logits.is_empty() {
        return Vec::new();
    }
    let t = temperature.max(1e-4);
    let scaled: Vec<f32> = logits.iter().map(|z| z / t).collect();
    let max = scaled.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let exp: Vec<f32> = scaled.iter().map(|z| (z - max).exp()).collect();
    let sum: f32 = exp.iter().sum::<f32>().max(1e-12);
    exp.into_iter().map(|v| v / sum).collect()
}

pub fn confidence_from_probs(probs: &[f32]) -> f32 {
    let k = probs.len();
    if k < 2 {
        return 1.0;
    }
    let ent: f32 = probs
        .iter()
        .map(|p| {
            let p = p.clamp(1e-12, 1.0);
            -p * p.ln()
        })
        .sum();
    (1.0 - ent / (k as f32).ln()).clamp(0.0, 1.0)
}

#[derive(Debug, Clone)]
pub struct QuestionSpec {
    pub id: &'static str,
    pub qtype: i64,
    pub instructions: &'static str,
    pub options: Vec<(&'static str, Option<&'static str>)>,
}

impl QuestionSpec {
    pub fn render_options(&self) -> Vec<String> {
        if self.qtype == QTYPE_NOUL {
            return vec![
                "false: no, the statement does not hold".into(),
                "true: yes, the statement holds".into(),
            ];
        }
        self.options
            .iter()
            .map(|(key, desc)| match desc {
                Some(text) if !text.is_empty() => format!("{key}: {text}"),
                _ => (*key).to_string(),
            })
            .collect()
    }
}

pub trait TokenEncode {
    fn encode(&self, text: &str) -> Vec<u32>;
    fn cls_id(&self) -> u32;
    fn sep_id(&self) -> u32;
    fn mask_id(&self) -> u32;
    fn pad_id(&self) -> u32;
    fn mask_token(&self) -> &str {
        "[MASK]"
    }
}

#[derive(Debug, Clone)]
pub struct PackedQuestion {
    pub id: String,
    pub qtype: i64,
    pub option_keys: Vec<String>,
    pub ids: Vec<u32>,
    pub markers: Vec<usize>,
}

pub fn build_sequence(
    tok: &impl TokenEncode,
    state: &str,
    question: &QuestionSpec,
    max_len: usize,
    head_max_len: usize,
) -> PackedQuestion {
    let mask = tok.mask_token();
    let opts = question.render_options();
    let ins = question.instructions.replace(mask, " ");
    let mut head_ids = tok.encode(&format!("{} question: {ins}", qtype_name(question.qtype)));
    let mut opt_ids: Vec<Vec<u32>> = opts
        .iter()
        .map(|opt| {
            let text = format!(" {}", opt.replace(mask, " "));
            let mut ids = vec![tok.mask_id()];
            ids.extend(tok.encode(&text).into_iter().take(48));
            ids
        })
        .collect();
    let mut opt_budget = head_max_len.saturating_sub(opt_ids.iter().map(Vec::len).sum());
    if opt_budget < 16 {
        let per = (head_max_len.saturating_sub(16) / opt_ids.len().max(1)).max(4);
        for ids in &mut opt_ids {
            ids.truncate(per);
        }
        opt_budget = head_max_len.saturating_sub(opt_ids.iter().map(Vec::len).sum());
    }
    head_ids.truncate(opt_budget.max(8));

    let mut ids = vec![tok.cls_id()];
    ids.extend(head_ids);
    ids.push(tok.sep_id());
    let mut markers = Vec::new();
    for opt in opt_ids {
        markers.push(ids.len());
        ids.extend(opt);
        ids.push(tok.sep_id());
    }
    let room = max_len.saturating_sub(ids.len() + 1);
    let mut state_ids = tok.encode(&state.replace(mask, " "));
    state_ids.truncate(room);
    ids.extend(state_ids);
    ids.push(tok.sep_id());
    ids.truncate(max_len);
    let markers: Vec<usize> = markers.into_iter().filter(|m| *m < max_len).collect();
    PackedQuestion {
        id: question.id.into(),
        qtype: question.qtype,
        option_keys: question
            .options
            .iter()
            .map(|(k, _)| (*k).to_string())
            .collect(),
        ids,
        markers,
    }
}

fn qtype_name(qtype: i64) -> &'static str {
    match qtype {
        QTYPE_SCORE => "score",
        QTYPE_NOUL => "noul",
        _ => "choice",
    }
}

#[derive(Debug, Clone)]
pub struct PackedBatch {
    pub items: Vec<PackedQuestion>,
    pub input_ids: Vec<u32>,
    pub attention_mask: Vec<u32>,
    pub marker_pos: Vec<u32>,
    pub marker_mask: Vec<u32>,
    pub qtypes: Vec<i64>,
    pub batch: usize,
    pub seq: usize,
    pub kmax: usize,
}

impl PackedBatch {
    pub fn from_questions(items: Vec<PackedQuestion>, pad_id: u32) -> Self {
        let batch = items.len();
        let seq = items.iter().map(|item| item.ids.len()).max().unwrap_or(0);
        let kmax = items
            .iter()
            .map(|item| item.markers.len())
            .max()
            .unwrap_or(0);
        let mut input_ids = vec![pad_id; batch * seq];
        let mut attention_mask = vec![0u32; batch * seq];
        let mut marker_pos = vec![0u32; batch * kmax];
        let mut marker_mask = vec![0u32; batch * kmax];
        let mut qtypes = Vec::with_capacity(batch);
        for (i, item) in items.iter().enumerate() {
            qtypes.push(item.qtype);
            for (t, id) in item.ids.iter().enumerate() {
                input_ids[i * seq + t] = *id;
                attention_mask[i * seq + t] = 1;
            }
            for (k, pos) in item.markers.iter().enumerate() {
                marker_pos[i * kmax + k] = *pos as u32;
                marker_mask[i * kmax + k] = 1;
            }
        }
        Self {
            items,
            input_ids,
            attention_mask,
            marker_pos,
            marker_mask,
            qtypes,
            batch,
            seq,
            kmax,
        }
    }
}

pub struct HfTokenizer {
    inner: tokenizers::Tokenizer,
    cls: u32,
    sep: u32,
    mask: u32,
    pad: u32,
}

impl HfTokenizer {
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self> {
        let inner = tokenizers::Tokenizer::from_file(path.as_ref())
            .map_err(|err| anyhow::anyhow!("tokenizer.json: {err}"))?;
        let id = |token: &str, fallback: u32| inner.token_to_id(token).unwrap_or(fallback);
        Ok(Self {
            cls: id("[CLS]", 50281),
            sep: id("[SEP]", 50282),
            mask: id("[MASK]", 50284),
            pad: id("[PAD]", 50283),
            inner,
        })
    }
}

impl TokenEncode for HfTokenizer {
    fn encode(&self, text: &str) -> Vec<u32> {
        self.inner
            .encode(text, false)
            .map(|enc| enc.get_ids().to_vec())
            .unwrap_or_default()
    }

    fn cls_id(&self) -> u32 {
        self.cls
    }

    fn sep_id(&self) -> u32 {
        self.sep
    }

    fn mask_id(&self) -> u32 {
        self.mask
    }

    fn pad_id(&self) -> u32 {
        self.pad
    }
}

struct MultiHead {
    in_proj: Linear,
    out_proj: Linear,
    nhead: usize,
}

impl MultiHead {
    fn load(vb: VarBuilder, hidden: usize) -> CandleResult<Self> {
        let nhead = (hidden / 64).max(1);
        let weight = vb.get((hidden * 3, hidden), "in_proj_weight")?;
        let bias = vb.get(hidden * 3, "in_proj_bias")?;
        Ok(Self {
            in_proj: Linear::new(weight, Some(bias)),
            out_proj: linear(hidden, hidden, vb.pp("out_proj"))?,
            nhead,
        })
    }

    fn forward(&self, xs: &Tensor, keep_mask: &Tensor) -> CandleResult<Tensor> {
        let (b, s, e) = xs.dims3()?;
        let head = e / self.nhead;
        let qkv = xs.apply(&self.in_proj)?;
        let chunks = qkv.chunk(3, D::Minus1)?;
        let to_heads = |t: Tensor| -> CandleResult<Tensor> {
            t.reshape((b, s, self.nhead, head))?.transpose(1, 2)
        };
        let q = to_heads(chunks[0].clone())?;
        let k = to_heads(chunks[1].clone())?;
        let v = to_heads(chunks[2].clone())?;
        let scale = (head as f64).sqrt();
        let att = (q.matmul(&k.transpose(D::Minus2, D::Minus1)?)? / scale)?;
        let keep = keep_mask.to_dtype(DType::F32)?.unsqueeze(1)?.unsqueeze(2)?;
        let inverted = (1.0 - keep)?;
        let att = att.broadcast_add(&(inverted * f32::MIN as f64)?)?;
        let att = softmax(&att, D::Minus1)?;
        let xs = att.matmul(&v)?.transpose(1, 2)?.reshape((b, s, e))?;
        xs.apply(&self.out_proj)
    }
}

struct EncoderLayer {
    attn: MultiHead,
    linear1: Linear,
    linear2: Linear,
    norm1: LayerNorm,
    norm2: LayerNorm,
}

impl EncoderLayer {
    fn load(vb: VarBuilder, hidden: usize) -> CandleResult<Self> {
        Ok(Self {
            attn: MultiHead::load(vb.pp("self_attn"), hidden)?,
            linear1: linear(hidden, hidden * 4, vb.pp("linear1"))?,
            linear2: linear(hidden * 4, hidden, vb.pp("linear2"))?,
            norm1: layer_norm(hidden, 1e-5, vb.pp("norm1"))?,
            norm2: layer_norm(hidden, 1e-5, vb.pp("norm2"))?,
        })
    }

    fn forward(&self, xs: &Tensor, keep_mask: &Tensor) -> CandleResult<Tensor> {
        let sa = self.attn.forward(&xs.apply(&self.norm1)?, keep_mask)?;
        let xs = (xs + sa)?;
        let ff = xs
            .apply(&self.norm2)?
            .apply(&self.linear1)?
            .relu()?
            .apply(&self.linear2)?;
        xs + ff
    }
}

struct Scorer {
    norm: LayerNorm,
    dense: Linear,
    out: Linear,
}

impl Scorer {
    fn load(vb: VarBuilder, hidden: usize) -> CandleResult<Self> {
        Ok(Self {
            norm: layer_norm(hidden, 1e-5, vb.pp("0"))?,
            dense: linear(hidden, hidden, vb.pp("1"))?,
            out: linear(hidden, 1, vb.pp("3"))?,
        })
    }

    fn forward(&self, xs: &Tensor) -> CandleResult<Tensor> {
        xs.apply(&self.norm)?
            .apply(&self.dense)?
            .gelu_erf()?
            .apply(&self.out)
    }
}

pub struct DecisionModel {
    encoder: ModernBert,
    type_emb: Embedding,
    head: Vec<EncoderLayer>,
    scorer: Scorer,
    act_in: Linear,
    act_out: Linear,
}

impl DecisionModel {
    pub fn load(vb: VarBuilder, bert: &BertConfig) -> CandleResult<Self> {
        let hidden = bert.hidden_size;
        Ok(Self {
            encoder: ModernBert::load(vb.clone(), bert)?,
            type_emb: embedding(3, hidden, vb.pp("type_emb"))?,
            head: vec![
                EncoderLayer::load(vb.pp("head.layers.0"), hidden)?,
                EncoderLayer::load(vb.pp("head.layers.1"), hidden)?,
            ],
            scorer: Scorer::load(vb.pp("scorer"), hidden)?,
            act_in: linear(hidden + 4, 256, vb.pp("act_head.0"))?,
            act_out: linear(256, 2, vb.pp("act_head.2"))?,
        })
    }

    pub fn forward(
        &self,
        input_ids: &Tensor,
        attention_mask: &Tensor,
        marker_pos: &[Vec<u32>],
        marker_mask: &[Vec<u32>],
        qtype: &Tensor,
    ) -> CandleResult<(Vec<Vec<f32>>, Tensor)> {
        let mut h = self.encoder.forward(input_ids, attention_mask)?;
        let type_add = qtype.apply(&self.type_emb)?.unsqueeze(1)?;
        h = h.broadcast_add(&type_add)?;
        for layer in &self.head {
            h = layer.forward(&h, attention_mask)?;
        }
        let (b, _, d) = h.dims3()?;
        let mut logits = Vec::with_capacity(b);
        for bi in 0..b {
            let row = h.i(bi)?;
            let mut scores = Vec::new();
            for (k, pos) in marker_pos[bi].iter().enumerate() {
                if marker_mask[bi].get(k).copied().unwrap_or(0) == 0 {
                    scores.push(-1e4);
                    continue;
                }
                let hidden = row.i(*pos as usize)?;
                let score = self
                    .scorer
                    .forward(&hidden.unsqueeze(0)?)?
                    .squeeze(0)?
                    .squeeze(0)?;
                scores.push(score.to_vec0::<f32>()?);
            }
            logits.push(scores);
        }
        let pooled = h.i((.., 0, ..))?.contiguous()?.to_dtype(DType::F32)?;
        let feats = act_features(&logits, marker_mask, d, pooled.device())?;
        let act = Tensor::cat(&[&pooled, &feats], D::Minus1)?
            .apply(&self.act_in)?
            .gelu_erf()?
            .apply(&self.act_out)?;
        Ok((logits, act))
    }
}

fn act_features(
    logits: &[Vec<f32>],
    marker_mask: &[Vec<u32>],
    _hidden: usize,
    device: &Device,
) -> CandleResult<Tensor> {
    let mut rows = Vec::with_capacity(logits.len() * 4);
    for (i, z) in logits.iter().enumerate() {
        let k = marker_mask
            .get(i)
            .map(|m| m.iter().filter(|v| **v == 1).count())
            .unwrap_or(0)
            .max(2) as f32;
        let valid: Vec<f32> = z
            .iter()
            .zip(marker_mask.get(i).into_iter().flatten())
            .filter_map(|(v, m)| (*m == 1).then_some(*v))
            .collect();
        let p = softmax_temp(&valid, 1.0);
        let mut ranked = p.clone();
        ranked.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
        let top1 = ranked.first().copied().unwrap_or(0.0);
        let top2 = ranked.get(1).copied().unwrap_or(0.0);
        let ent: f32 = if p.len() < 2 {
            0.0
        } else {
            let raw: f32 = p
                .iter()
                .map(|prob| {
                    let prob = prob.clamp(1e-9, 1.0);
                    -prob * prob.ln()
                })
                .sum();
            raw / k.ln()
        };
        rows.extend_from_slice(&[top1, top1 - top2, ent, k / 255.0]);
    }
    Tensor::from_vec(rows, (logits.len(), 4), device)
}

pub struct LayaSession {
    pub device_name: String,
    pub cfg: LayaRuntimeConfig,
    pub tokenizer: HfTokenizer,
    model: DecisionModel,
    device: Device,
}

impl LayaSession {
    pub fn load(dir: &Path) -> Result<Self> {
        let cfg = load_runtime_config(&dir.join("rl_agent_config.json"))?;
        let tokenizer = HfTokenizer::from_file(dir.join("tokenizer/tokenizer.json"))?;
        let bert = load_bert_config(&dir.join("encoder/config.json"))?;
        let (device, device_name, dtype) = select_device();
        let weights = dir.join("model.safetensors");
        let tensors = candle_core::safetensors::load(&weights, &device)
            .with_context(|| format!("safetensors: {}", weights.display()))?;
        let remapped: HashMap<String, Tensor> = tensors
            .into_iter()
            .map(|(key, tensor)| (remap_weight_key(&key), tensor))
            .collect();
        let vb = VarBuilder::from_tensors(remapped, dtype, &device);
        let model = DecisionModel::load(vb, &bert).map_err(|err| anyhow::anyhow!("{err}"))?;
        Ok(Self {
            device_name,
            cfg,
            tokenizer,
            model,
            device,
        })
    }

    pub fn infer_batch(&self, items: Vec<PackedQuestion>) -> Result<Vec<Vec<f32>>> {
        let pad = self.tokenizer.pad_id();
        let batch = PackedBatch::from_questions(items, pad);
        if batch.batch == 0 || batch.seq == 0 {
            anyhow::bail!("boş Laya batch");
        }
        let input = Tensor::from_vec(
            batch.input_ids.clone(),
            (batch.batch, batch.seq),
            &self.device,
        )?;
        let mask = Tensor::from_vec(
            batch.attention_mask.clone(),
            (batch.batch, batch.seq),
            &self.device,
        )?;
        let qtype_ids: Vec<u32> = batch.qtypes.iter().map(|q| *q as u32).collect();
        let qtype = Tensor::from_vec(qtype_ids, batch.batch, &self.device)?;
        let mut marker_pos = vec![vec![0u32; batch.kmax]; batch.batch];
        let mut marker_mask = vec![vec![0u32; batch.kmax]; batch.batch];
        for i in 0..batch.batch {
            for k in 0..batch.kmax {
                marker_pos[i][k] = batch.marker_pos[i * batch.kmax + k];
                marker_mask[i][k] = batch.marker_mask[i * batch.kmax + k];
            }
        }
        let (logits, _act) = self
            .model
            .forward(&input, &mask, &marker_pos, &marker_mask, &qtype)
            .map_err(|err| anyhow::anyhow!("Laya forward: {err}"))?;
        Ok(logits)
    }
}

pub fn remap_weight_key(key: &str) -> String {
    if let Some(rest) = key.strip_prefix("encoder.") {
        format!("model.{rest}")
    } else {
        key.to_string()
    }
}

fn select_device() -> (Device, String, DType) {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        if let Ok(device) = Device::new_metal(0) {
            return (device, "metal".into(), DType::F16);
        }
    }
    (Device::Cpu, "cpu".into(), DType::F32)
}

#[derive(Deserialize)]
struct RuntimeFile {
    #[serde(default = "default_max_len")]
    max_len: usize,
    #[serde(default = "default_head_max")]
    head_max_len: usize,
    #[serde(default)]
    temperature: Vec<f32>,
    #[serde(default)]
    temperature_by_options: HashMap<String, f32>,
}

fn default_max_len() -> usize {
    MAX_LEN
}

fn default_head_max() -> usize {
    HEAD_MAX_LEN
}

fn load_runtime_config(path: &Path) -> Result<LayaRuntimeConfig> {
    if !path.exists() {
        return Ok(LayaRuntimeConfig::default());
    }
    let raw = std::fs::read_to_string(path)?;
    let file: RuntimeFile = serde_json::from_str(&raw)?;
    let mut cfg = LayaRuntimeConfig {
        max_len: file.max_len,
        head_max_len: file.head_max_len,
        ..LayaRuntimeConfig::default()
    };
    for (i, value) in file.temperature.iter().take(3).enumerate() {
        cfg.temperature[i] = *value;
    }
    if !file.temperature_by_options.is_empty() {
        cfg.temperature_by_options = file.temperature_by_options;
    }
    Ok(cfg)
}

#[derive(Deserialize)]
struct EncoderFile {
    vocab_size: usize,
    hidden_size: usize,
    num_hidden_layers: usize,
    num_attention_heads: usize,
    intermediate_size: usize,
    max_position_embeddings: usize,
    #[serde(default = "default_eps")]
    layer_norm_eps: f64,
    pad_token_id: u32,
    global_attn_every_n_layers: usize,
    local_attention: usize,
    #[serde(default)]
    rope_parameters: Option<RopeFile>,
}

#[derive(Deserialize)]
struct RopeFile {
    #[serde(default)]
    full_attention: Option<RopeTheta>,
    #[serde(default)]
    sliding_attention: Option<RopeTheta>,
}

#[derive(Deserialize)]
struct RopeTheta {
    rope_theta: f64,
}

fn default_eps() -> f64 {
    1e-5
}

fn load_bert_config(path: &Path) -> Result<BertConfig> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("encoder config: {}", path.display()))?;
    let file: EncoderFile = serde_json::from_str(&raw)?;
    Ok(BertConfig {
        vocab_size: file.vocab_size,
        hidden_size: file.hidden_size,
        num_hidden_layers: file.num_hidden_layers,
        num_attention_heads: file.num_attention_heads,
        intermediate_size: file.intermediate_size,
        max_position_embeddings: file.max_position_embeddings,
        layer_norm_eps: file.layer_norm_eps,
        pad_token_id: file.pad_token_id,
        global_attn_every_n_layers: file.global_attn_every_n_layers,
        global_rope_theta: file
            .rope_parameters
            .as_ref()
            .and_then(|r| r.full_attention.as_ref())
            .map(|r| r.rope_theta)
            .unwrap_or(160_000.0),
        local_attention: file.local_attention,
        local_rope_theta: file
            .rope_parameters
            .as_ref()
            .and_then(|r| r.sliding_attention.as_ref())
            .map(|r| r.rope_theta)
            .unwrap_or(10_000.0),
        classifier_config: None,
    })
}

#[derive(Debug, Clone)]
pub struct HashTokenizer {
    pub cls: u32,
    pub sep: u32,
    pub mask: u32,
    pub pad: u32,
}

impl Default for HashTokenizer {
    fn default() -> Self {
        Self {
            cls: 1,
            sep: 2,
            mask: 3,
            pad: 0,
        }
    }
}

impl TokenEncode for HashTokenizer {
    fn encode(&self, text: &str) -> Vec<u32> {
        text.split_whitespace()
            .map(|word| {
                let mut h = 17u32;
                for b in word.bytes() {
                    h = h.wrapping_mul(31).wrapping_add(u32::from(b));
                }
                (h % 500) + 10
            })
            .collect()
    }

    fn cls_id(&self) -> u32 {
        self.cls
    }

    fn sep_id(&self) -> u32 {
        self.sep
    }

    fn mask_id(&self) -> u32 {
        self.mask
    }

    fn pad_id(&self) -> u32 {
        self.pad
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn routing_q() -> QuestionSpec {
        QuestionSpec {
            id: "ROUTING_TYPE",
            qtype: QTYPE_CHOICE,
            instructions: "What kind of lounge message is this?",
            options: vec![
                ("Task", Some("work order")),
                ("Experience", Some("memory")),
                ("Review", Some("code review")),
            ],
        }
    }

    #[test]
    fn packs_one_mask_per_option() {
        let packed = build_sequence(
            &HashTokenizer::default(),
            "subject lounge.task.requested",
            &routing_q(),
            128,
            64,
        );
        assert_eq!(packed.markers.len(), 3);
        assert_eq!(packed.ids[packed.markers[0]], 3);
        assert_eq!(packed.ids[packed.markers[1]], 3);
        assert_eq!(packed.ids[packed.markers[2]], 3);
        assert!(packed.ids.first().copied() == Some(1));
        assert!(packed.ids.last().copied() == Some(2));
    }

    #[test]
    fn choice_3_5_uses_config_temperature() {
        let cfg = LayaRuntimeConfig::default();
        assert_eq!(temp_bucket(QTYPE_CHOICE, 3), "choice:3-5");
        let temp = temperature_for(&cfg, QTYPE_CHOICE, 3);
        assert!((temp - 1.760_151_9).abs() < 1e-5);
        let probs = softmax_temp(&[0.0, 0.0, 0.0], temp);
        assert_eq!(probs.len(), 3);
        assert!((probs.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        assert!((probs[0] - 1.0 / 3.0).abs() < 1e-5);
    }

    #[test]
    fn remaps_encoder_prefix_to_candle_model() {
        assert_eq!(
            remap_weight_key("encoder.embeddings.tok_embeddings.weight"),
            "model.embeddings.tok_embeddings.weight"
        );
        assert_eq!(remap_weight_key("scorer.1.weight"), "scorer.1.weight");
    }
}
