use super::*;
use crate::{
    scorecard::{ScorecardReading, prepare_score},
    scoring::{ScoreKind, ScoreValue},
};

mod support {
    use crate as wodbuster_jueces;
    include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/mod.rs"));
}

async fn form_command(page: &Page, request: Value) -> anyhow::Result<String> {
    let script = format!(
        "(async()=>{{const request={};{} }})()",
        request,
        include_str!("../score_form.js")
    );
    let result = page.evaluate(script).await?.into_value::<String>()?;
    if result == "pending" {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if page.evaluate("!window.busy").await?.into_value::<bool>()? {
                    break;
                }
                sleep(Duration::from_millis(10)).await;
            }
            Ok::<_, anyhow::Error>(())
        })
        .await??;
    }
    Ok(result)
}

#[tokio::test]
#[ignore = "Requiere Browserless; usa solo un formulario local, sin acceder a Arena"]
async fn refreshed_saved_and_unsaved_scores_prepare_without_saving_or_changing_original_values()
-> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let config = crate::config::Config::from_env().map_err(|e| anyhow::anyhow!("{e}"))?;
    let (browser, mut handler) = Browser::connect(config.browser_url.as_str()).await?;
    let _driver = Driver(tokio::spawn(async move {
        while handler.next().await.is_some() {}
    }));
    let context = browser
        .create_browser_context(CreateBrowserContextParams {
            dispose_on_detach: Some(true),
            ..Default::default()
        })
        .await?;
    let page = browser
        .new_page(
            CreateTargetParams::builder()
                .url("about:blank")
                .browser_context_id(context.clone())
                .build()
                .unwrap(),
        )
        .await?;
    let result = async {
        page.set_content(r#"
            <div id="forms"></div>
            <script>
            window.saves=0; window.edits=[]; window.busy=false; window.callbacks=[];
            const names=['Giralda','Saved reps','New reps'];
            const saved=['07:20','52reps',null];
            let active=-1;
            let completed=true;
            window.Sys={WebForms:{PageRequestManager:{getInstance:()=>({
                add_endRequest:cb=>callbacks.push(cb),
                remove_endRequest:cb=>callbacks=callbacks.filter(c=>c!==cb),
                get_isInAsyncPostBack:()=>busy
            })}}};
            function render(){
                document.getElementById('forms').innerHTML=names.map((name,index)=>{
                    const prefix=`w${index}`;
                    const readonly=saved[index]!==null && active!==index;
                    const button=readonly?'CtlEditar':'CtlSave';
                    let fields='';
                    if(!readonly){
                        if(index===0){
                            fields=`<select name="${prefix}$IoComplete" id="${prefix}_IoComplete" onchange="changeMode(this)">
                                <option value="True" ${completed?'selected':''}>Yes</option>
                                <option value="False" ${completed?'':'selected'}>No</option></select>`;
                            fields+=completed
                                ? `<input name="${prefix}$IoTimeMMSS" id="${prefix}_IoTimeMMSS" value="07:20">`
                                : `<input name="${prefix}$IoReps" id="${prefix}_IoReps" value="0">`;
                        }else fields=`<input name="${prefix}$IoReps" id="${prefix}_IoReps" value="${index===1?'52':''}">`;
                    }
                    return `<h4>${name}</h4><div id="CtlWodsResults_${index}_CtlUpMain">${fields}
                        <input type="submit" name="${prefix}$${button}" id="${prefix}_${button}_0"
                            onclick="${readonly?`editScore(${index})`:'window.saves++'};return false">
                        ${readonly?`<div class="athleteresult"><div class="athlete"><div class="affiliate">${saved[index]}</div></div></div>`:''}
                        </div>`;
                }).join('');
            }
            function postback(change){
                busy=true;
                setTimeout(()=>{change();render();busy=false;[...callbacks].forEach(cb=>cb(null,{get_error:()=>null}));},30);
            }
            function editScore(index){edits.push(index);postback(()=>{active=index;completed=true;});}
            function changeMode(select){const value=select.value==='True';postback(()=>{completed=value;});}
            render();
            </script>
        "#).await?;
        let mut detail = support::fixture();
        let original_wod = detail.categories[0].wods[0].clone();
        let mut team = detail.categories[0].teams[0].clone();
        let original_score = team.scores[0].clone();
        team.scores = (0..3).map(|index| {
            let mut score=original_score.clone();
            score.wod_id=64459+index;
            score.result=match index {0=>Some("07:20".into()),1=>Some("52reps".into()),_=>None};
            score.metadata=json!({});
            score
        }).collect();
        let mut wods:BTreeMap<_,_> = ["Giralda","Saved reps","New reps"].into_iter().enumerate().map(|(index,name)|{
            let mut wod=original_wod.clone();wod.id=64459+index as i64;wod.name=name.into();
            (wod.id,wod)
        }).collect();
        let original_results:Vec<_> = team.scores.iter().map(|score|score.result.clone()).collect();
        let initial = page.content().await?;
        details::enrich_judge_form(&initial,&mut team,&mut wods);
        assert_eq!(team.scores[0].metadata["save_controls"][0]["name"],"w0$CtlEditar");
        assert_eq!(team.scores[1].metadata["save_controls"][0]["name"],"w1$CtlEditar");
        assert_eq!(team.scores[2].metadata["save_controls"][0]["name"],"w2$CtlSave");
        capture_score_editors(&page,&mut team,&mut wods).await?;
        assert_eq!(team.scores.iter().map(|score|score.result.clone()).collect::<Vec<_>>(),original_results);
        for index in 0..3 {
            assert_eq!(team.scores[index].metadata["save_controls"][0]["name"],format!("w{index}$CtlSave"));
        }
        // Explorar False no sustituye el valor original True ni el tiempo guardado.
        assert_eq!(team.scores[0].metadata["fields"][0]["value"],"True");
        assert_eq!(team.scores[0].metadata["fields"][1]["value"],"07:20");
        detail.categories[0].teams=vec![team];
        detail.categories[0].wods=wods.into_values().collect();
        crate::scoring::prepare_detail(&mut detail);
        assert_eq!(detail.categories[0].wods[0].score_format.as_ref().unwrap().kind,ScoreKind::TimeOrRepetitions);
        assert_eq!(detail.categories[0].teams[0].scores[0].value,Some(ScoreValue::Completed {
            time:crate::scoring::MinutesSeconds {minutes:7,seconds:20},tie_break:None
        }));
        assert!(detail.categories[0].teams[0].scores[2].value.is_none());
        // Las dos ramas del WOD guardado y el WOD nuevo tienen referencias reales.
        for (index,value) in [
            (0,ScoreValue::Completed {time:crate::scoring::MinutesSeconds {minutes:8,seconds:10},tie_break:None}),
            (0,ScoreValue::NotCompleted {repetitions:2,tie_break:None}),
            (1,ScoreValue::Repetitions {repetitions:7}),
            (2,ScoreValue::Repetitions {repetitions:3}),
        ] {
            let reading=ScorecardReading {
                category_id:Some(16907),wod_id:Some(64459+index),team_reference:Some("298682".into()),
                score_value:Some(value),judge_name:None,needs_retake:false,retake_reason:None,
            };
            let mut plan=prepare_score(&detail,&reading).map_err(anyhow::Error::msg)?;
            plan.form.url="about:blank".into();
            form_command(&page,json!({"operation":"open-edit","plan":plan})).await?;
            for step in &plan.form.steps {
                form_command(&page,json!({"operation":"step","plan":plan,"step":step})).await?;
            }
            assert_eq!(form_command(&page,json!({"operation":"verify","plan":plan})).await?,"ready");
        }
        assert_eq!(page.evaluate("window.saves").await?.into_value::<u64>()?,0);
        assert!(schema_command(&page,&json!({"name":"w2$CtlSave","id":"w2_CtlSave_0"}),"New reps",None).await.is_err());
        assert_eq!(page.evaluate("window.saves").await?.into_value::<u64>()?,0);
        Ok(())
    }.await;
    browser.dispose_browser_context(context).await?;
    result
}
