use std::io::{BufReader, BufWriter, Cursor, Write};
use std::path::PathBuf;
use std::{collections::HashMap, fs::File, io::Read, ops::Range};

use anyhow::{bail, Context};
use serde::Deserialize;
use structopt::StructOpt;
use vibrato::{dictionary::WordIdx, Dictionary, Tokenizer};

static HTML_HEAD: &'static str = r#"<!DOCTYPE html>
<html lang="ja">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <style>
        body {
            color: #ddd;
            background-color: #333;
            font-size: 16px;
            line-height: 28px;
            max-width: 584px; /* 16px * 35 + 12px * 2 */
            margin-left: auto;
            margin-right: auto;
        }
        .novel-body {
            white-space: pre-wrap;
            line-break: strict;
            padding-left: 12px;
            padding-right: 12px;
            font-family: '游明朝',YuMincho,'ヒラギノ明朝 Pr6N','Hiragino Mincho Pr6N','ヒラギノ明朝 ProN','Hiragino Mincho ProN','ヒラギノ明朝 StdN','Hiragino Mincho StdN',HiraMinProN-W3,'HGS明朝B','HG明朝B','Helvetica Neue',Helvetica,Arial,'ヒラギノ角ゴ Pr6N','Hiragino Kaku Gothic Pr6N','ヒラギノ角ゴ ProN','Hiragino Kaku Gothic ProN','ヒラギノ角ゴ StdN','Hiragino Kaku Gothic StdN','Segoe UI',Verdana,'メイリオ',Meiryo,sans-serif;
        }
        .separator {
            color: #444;
        }
        .alert {
            background-color: #933;
        }
    </style>
</head>
<body>

<div class="novel-body">
"#;

static HTML_FOOT: &'static str = r#"
</div>
</body>
</html>
"#;

#[derive(Debug, structopt::StructOpt)]
struct Opt {
    #[structopt(name = "INPUT_FILE", parse(from_os_str))]
    pub input: PathBuf,

    #[structopt(name = "OUTPUT_FILE", short = "o", parse(from_os_str))]
    pub output: Option<PathBuf>,

    #[structopt(name = "PROJECT_ROOT", short = "r", parse(from_os_str))]
    pub project_root: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
struct Config {
    distance_threshold: usize,
    proper_nouns: Vec<String>,
    ignores: Vec<String>,
}

#[derive(Debug)]
struct Annotation {
    distance: usize,
}

fn read_user_dict(config: &Config) -> anyhow::Result<impl Read> {
    let mut buf = String::new();
    for noun in &config.proper_nouns {
        buf.push_str(&format!("{},1293,1293,0,固有名詞\n", noun));
    }
    buf.truncate(buf.trim_end().len());
    Ok(BufReader::new(Cursor::new(buf)))
}

fn main() -> anyhow::Result<()> {
    let opt = Opt::from_args();

    let exe_dir = match opt.project_root {
        Some(p) => p,
        None => {
            let exe_path = std::env::current_exe()?;
            let Some(exe_dir) = exe_path.parent() else {
                bail!("failed to get exe directory");
            };
            exe_dir.to_owned()
        },
    };

    let config_path = exe_dir.join("config.toml");
    let config: Config = {
        let mut file = File::open(config_path).context("failed to open config")?;
        let mut buf = String::new();
        file.read_to_string(&mut buf)?;
        toml::from_str(&buf)?
    };

    let dict_path = exe_dir.join("dict/bccwj-suw+unidic-cwj-3_1_1+compact/system.dic.zst");
    let tokenizer = {
        let reader = zstd::Decoder::new(File::open(dict_path).context("failed to open dict")?)?;
        let mut dict = Dictionary::read(reader)?;
        let user_dict = read_user_dict(&config)?;
        dict = dict.reset_user_lexicon_from_reader(Some(user_dict))?;
        Tokenizer::new(dict).max_grouping_len(24)
    };

    let mut worker = tokenizer.new_worker();

    let mut text = String::new();
    File::open(opt.input)?.read_to_string(&mut text)?;

    worker.reset_sentence(text);
    worker.tokenize();

    println!("Tokenized. num_tokens={}", worker.num_tokens());

    // 転置インデックス
    let mut index: HashMap<WordIdx, Vec<Range<usize>>> = HashMap::new();
    let mut surfaces: HashMap<WordIdx, String> = HashMap::new();

    // テキストをスキャンして転置インデックスに入れていく
    for token in worker.token_iter() {
        let pos = match token.feature().split_once(',') {
            Some((pos, _)) => pos,
            None => token.feature(),
        };
        if config.ignores.contains(&pos.to_owned()) {
            continue;
        }
        index
            .entry(token.word_idx())
            .or_default()
            .push(token.range_char());
        surfaces
            .entry(token.word_idx())
            .or_insert(token.surface().to_owned());
    }

    let mut annotations: HashMap<usize, Annotation> = HashMap::new();

    // 転置インデックスをスキャンして近くに同じ単語が出現していないか調べる
    for (_, ranges) in index.iter() {
        //let surface = &surfaces[word_idx];
        //let features = tokenizer.dictionary().word_feature(*word_idx);
        for i in 0..ranges.len() {
            let ri = &ranges[i];
            let mut min_dist = usize::MAX;
            for j in 0..ranges.len() {
                if i == j {
                    continue;
                }
                let rj = &ranges[j];
                let dist = usize::min(
                    usize::abs_diff(ri.start, rj.end),
                    usize::abs_diff(ri.end, rj.start),
                );
                min_dist = min_dist.min(dist);
            }
            annotations.insert(ri.start, Annotation { distance: min_dist });
        }
    }

    // HTMLを出力する
    let html_path = opt.output.unwrap_or(exe_dir.join("target/result.html"));
    let html_file = File::create(&html_path).context("failed to open output file")?;
    let mut writer = BufWriter::new(html_file);
    write!(writer, "{}", HTML_HEAD)?;

    for token in worker.token_iter() {
        let annotation = annotations.get(&token.range_char().start);
        let class = if annotation.is_some_and(|a| a.distance < config.distance_threshold) {
            "alert"
        } else {
            ""
        };
        write!(
            writer,
            "<span title='{}' class='{}'>",
            html_escape::encode_text(token.feature()),
            class
        )?;
        write!(writer, "{}", html_escape::encode_text(token.surface()))?;
        write!(writer, "</span>")?;
        write!(writer, "<span class='separator'>|</span>")?;
    }

    write!(writer, "{}", HTML_FOOT)?;
    writer.flush()?;
    drop(writer);

    println!("Wrote result to '{}'.", html_path.to_string_lossy());

    webbrowser::open(&html_path.to_string_lossy())?;

    Ok(())
}
