//! The interactive client: the org's active pull requests, and a page per
//! pull request with its overview, changed files, diffs and comments.

use std::{
	collections::{HashMap, HashSet},
	sync::{
		Arc,
		mpsc::{self, Receiver, Sender},
	},
	time::{Duration, Instant},
};

use tern_sdk::{
	Input, Options, Session, SurfaceOptions,
	keys::Key,
	ui::{self, Hunk, Side, Tone, View, html},
	wire::{self, Event, RevealAt, ScrollBy, feature},
};

use crate::{
	review,
	api::{self, Client, Detail, Identity, Pr, Thread},
	config,
	diff::{self, FileDiff},
	tree,
	worker::{self, Job, Reply, Sides},
};

/// CSS pixels per unit of the cell height Tern reports (a text line is about 15.5px at the default size).
const CSS_PER_CELL_H: f64 = 0.78;
/// The height of one list row, in CSS pixels (matches `.row` below).
const ROW_PX: f64 = 22.0;
/// Context lines around each change; `x` shows the whole file.
const CONTEXT: usize = 3;
/// Lines of the file shown above a change when a jump scrolls to it.
const JUMP_LEAD: usize = 5;
/// The pane width (in columns) from which the navigation rail shows; below it `/` jumps between PRs.
const RAIL_MIN_COLS: f64 = 110.0;
/// Results the switcher shows at once.
const JUMP_ROWS: usize = 8;
const COMPOSER: &str = "dock.ed";
/// The comment box that opens under a diff line.
const LINE_BOX: &str = "main.pbody.fv.dp.ds.cbox";
const LINE_EDITOR: &str = "main.pbody.fv.dp.ds.cbox.ed";

const CSS: &str = "\
.box{height:100%;box-sizing:border-box;width:100%;overflow:hidden;padding:4px;border:1px solid color-mix(in srgb,var(--sf-c-muted) 45%,transparent);border-radius:6px;font-family:Geist,system-ui,sans-serif;font-size:12.9px}\
.scroll{overflow:auto;padding:6px 10px}\
.row{display:flex;align-items:center;gap:10px;height:22px;box-sizing:border-box;padding:0 8px;white-space:nowrap;border-radius:4px;cursor:pointer}\
.row:hover{background:color-mix(in srgb,var(--sf-c-muted) 14%,transparent)}\
.row.cur{background:color-mix(in srgb,var(--sf-c-accent) 24%,transparent)}\
.grp{color:var(--sf-c-muted);font-weight:600;cursor:default}.grp:hover{background:none}\
.pr{padding-left:22px;gap:8px}\
.ph{background:color-mix(in srgb,var(--sf-c-muted) 9%,transparent);font-weight:650;gap:6px}\
.ph.cur{background:color-mix(in srgb,var(--sf-c-accent) 24%,transparent)}\
.ph .az{width:12px;text-align:center;flex-shrink:0;color:var(--sf-c-muted)}\
.ph .rp{color:var(--sf-c-muted);font-weight:400}\
.ph .n{margin-left:auto;color:var(--sf-c-muted);font-weight:400;font-variant-numeric:tabular-nums}\
.cr{cursor:pointer;color:var(--sf-c-muted)}.cr:hover{color:var(--sf-c-accent);text-decoration:underline}.crn{font-variant-numeric:tabular-nums}\n.lw{display:flex;gap:10px;height:100%;width:100%}.lw .box{flex:1;min-width:0;width:auto}\n.rail{width:220px;flex-shrink:0;box-sizing:border-box;height:100%;overflow:hidden;padding:4px;border:1px solid color-mix(in srgb,var(--sf-c-muted) 45%,transparent);border-radius:6px;font-family:Geist,system-ui,sans-serif;font-size:12.9px}\n.rail .row{gap:6px}.rail .nm{flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis}.rail .n{color:var(--sf-c-muted);font-variant-numeric:tabular-nums}\n.lwm{flex:1;min-width:0;height:100%;overflow:hidden;display:flex;flex-direction:column;font-family:Geist,system-ui,sans-serif}\
.rail.full{width:100%}\
.pbb{flex:1;min-height:0;position:relative;overflow:hidden}\
.pbb>.ov,.pbb>.azfv,.pbb>.box,.pbb>.azmsg{position:absolute;top:0;left:0;right:0;bottom:0;height:auto}\
.shell{display:flex;flex-direction:column;height:100%;min-height:0}\
.shell .lw{flex:1;min-height:0;height:auto;overflow:hidden;align-items:stretch}.shell .lw>.rail{height:auto;align-self:stretch;min-height:0;overflow-y:auto;overflow-x:hidden}.shell>.lwm{flex:1;min-height:0;height:auto}.shell>.rail.full{flex:1;min-height:0;height:auto}\
.topbar{display:flex;flex-wrap:wrap;gap:6px 22px;align-items:center;flex-shrink:0;padding:2px 6px 10px;font-family:Geist,system-ui,sans-serif;font-size:12.9px}\
.tbg{display:flex;flex-wrap:wrap;gap:5px;align-items:center}\
.tbl{color:var(--sf-c-muted);font-size:11px;font-weight:600;text-transform:uppercase;letter-spacing:.04em;margin-right:2px}\
.tbc{padding:2px 10px;border-radius:12px;cursor:pointer;white-space:nowrap;border:1px solid color-mix(in srgb,var(--sf-c-muted) 35%,transparent)}\
.tbc:hover{border-color:var(--sf-c-accent)}\
.tbc.on{background:color-mix(in srgb,var(--sf-c-accent) 24%,transparent);border-color:var(--sf-c-accent)}\
.tbc .n{color:var(--sf-c-muted);margin-left:6px;font-variant-numeric:tabular-nums}\
.topbar{position:relative;z-index:5;gap:10px;padding-bottom:8px}\
.dd{position:relative}\
.ddb{display:flex;align-items:center;gap:8px;padding:4px 10px;border-radius:6px;cursor:pointer;white-space:nowrap;border:1px solid color-mix(in srgb,var(--sf-c-muted) 40%,transparent)}\
.ddb:hover,.ddb.on{border-color:var(--sf-c-accent)}\
.ddb .tbl{margin:0}.ddb .az{color:var(--sf-c-muted);font-size:10px}\
.ddm{position:absolute;top:calc(100% + 4px);left:0;min-width:100%;z-index:20;box-sizing:border-box;padding:4px;border-radius:6px;max-height:60vh;overflow-y:auto;background:var(--sf-c-bg,#1e1e22);border:1px solid var(--sf-c-accent);box-shadow:0 6px 18px rgba(0,0,0,.45)}\
.ddi{display:flex;align-items:center;gap:14px;padding:5px 10px;border-radius:4px;cursor:pointer;white-space:nowrap}\
.ddi:hover{background:color-mix(in srgb,var(--sf-c-muted) 16%,transparent)}\
.ddi.on{background:color-mix(in srgb,var(--sf-c-accent) 24%,transparent)}\
.ddi .n{margin-left:auto;color:var(--sf-c-muted);font-variant-numeric:tabular-nums}\
.ptabs{display:flex;gap:4px;margin:2px 4px 0;border-bottom:1px solid color-mix(in srgb,var(--sf-c-muted) 28%,transparent);flex-shrink:0}\
.ptab{padding:6px 12px;cursor:pointer;color:var(--sf-c-muted);border-bottom:2px solid transparent}\
.azcbf>.azbtn.ai+.azbtn.ai{margin-right:auto}\
.aipop{box-sizing:border-box;display:flex;flex-direction:column;gap:8px;padding:10px 12px;border-radius:8px;white-space:normal;background:var(--sf-c-bg,#1e1e22);border:1px solid var(--sf-c-accent);font-family:Geist,system-ui,sans-serif}\
.rvd>.aipop{position:absolute;top:calc(100% + 6px);right:0;width:640px;max-width:85vw;z-index:30;box-shadow:0 8px 24px rgba(0,0,0,.5)}\
.card>.aipop,.azcbox>.aipop{margin:8px 0 4px 36px}\
.aipop .apt{font-weight:600}.aipop .apd{color:var(--sf-c-muted);font-size:12px;word-break:break-all}.aipop .aph{font-size:11.5px}\
.aipop .aps{padding:5px 9px;border-radius:6px;border:1px solid color-mix(in srgb,var(--sf-c-muted) 45%,transparent);font-family:ui-monospace,Consolas,monospace;font-size:12.5px}\
.aipop .apl{max-height:300px;overflow-y:auto}.aipop .apf{display:flex;align-items:center;gap:8px}.aipop .apf .dim{margin-right:auto}\
.aipop .apr{display:flex}.pvx{max-height:220px;overflow-y:auto;white-space:pre-wrap;padding:6px 10px;border-radius:6px;font-size:12px;background:color-mix(in srgb,var(--sf-c-muted) 12%,transparent)}\
.rvd{margin-left:2px;align-self:center}.rvd .ddm{left:auto;right:0;min-width:230px}.rvg.on{color:inherit}\
.rvb.sec{margin-left:0;margin-right:2px;background:transparent;border-color:color-mix(in srgb,var(--sf-c-muted) 50%,transparent)}\
.cact{display:flex;gap:10px;padding:4px 0 0 36px}.cab{color:var(--sf-c-muted);cursor:pointer;font-size:12px}.cab:hover{color:var(--sf-c-accent)}\
.rvp{overflow:hidden;white-space:nowrap}.pvt{cursor:pointer;color:var(--sf-c-accent);font-weight:600}.pvt.on{text-decoration:underline}\
.pvb{height:100%;min-height:0;box-sizing:border-box;overflow-y:auto;white-space:pre-wrap;padding:6px 10px;border:1px solid var(--sf-c-accent);border-radius:6px;font-family:Geist,system-ui,sans-serif;font-size:12.5px}\
.rvb{margin-left:auto;align-self:center;padding:3px 12px;border-radius:6px;cursor:pointer;font-weight:600;background:color-mix(in srgb,var(--sf-c-accent) 22%,transparent);border:1px solid var(--sf-c-accent)}\
.rvb:hover{background:color-mix(in srgb,var(--sf-c-accent) 38%,transparent)}\
.rvg{align-self:center;padding:3px 8px;cursor:pointer;color:var(--sf-c-muted)}.rvg:hover{color:inherit}\
.ptab:hover{color:inherit}.ptab.on{color:inherit;font-weight:600;border-bottom-color:var(--sf-c-accent)}\
.head{flex-shrink:0}\
.jump{box-sizing:border-box;padding:4px;border:1px solid var(--sf-c-accent);border-radius:6px;font-family:Geist,system-ui,sans-serif;font-size:12.9px;overflow:hidden}\
.jump .pj{color:var(--sf-c-muted)}\
.rail .rp{padding-left:8px;gap:6px}.rail .ri{color:var(--sf-c-muted);font-variant-numeric:tabular-nums;min-width:40px}\
.rail{overflow-y:auto;width:330px}\
.rail .rl{padding:6px 8px 2px;color:var(--sf-c-muted);font-size:11px;font-weight:600;letter-spacing:.04em;text-transform:uppercase;cursor:default}\
.rpr{padding:5px 8px 6px 16px;border-radius:4px;cursor:pointer;line-height:1.3}.rpr:hover{background:color-mix(in srgb,var(--sf-c-muted) 14%,transparent)}\
.rpr.cur{background:color-mix(in srgb,var(--sf-c-accent) 24%,transparent)}\
.rpr .rt{font-size:14.5px;font-weight:550;line-height:1.3;display:-webkit-box;-webkit-line-clamp:3;-webkit-box-orient:vertical;overflow:hidden;white-space:normal}\
.rpr .rm{display:flex;flex-wrap:wrap;align-items:center;gap:1px 8px;margin-top:3px;color:var(--sf-c-muted);font-size:11.5px}\
.crumb{gap:8px;padding:2px 4px 0}\n.rh{padding-left:22px;color:var(--sf-c-muted);font-size:11.5px;font-weight:600;cursor:default}.rh:hover{background:none}\
.id{color:var(--sf-c-muted);min-width:46px;font-variant-numeric:tabular-nums}\
.t{flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis}\
.au{width:96px;flex-shrink:0;overflow:hidden;text-overflow:ellipsis;color:var(--sf-c-muted)}\
.rb{width:230px;flex-shrink:0;overflow:hidden;text-overflow:ellipsis;color:var(--sf-c-muted);font-family:\"Geist Mono\",ui-monospace,monospace;font-size:11.5px}\
.ag{width:30px;flex-shrink:0;text-align:right;color:var(--sf-c-muted);font-variant-numeric:tabular-nums}\
.ok{color:var(--sf-good,#3fb950)}.bad{color:var(--sf-bad,#f85149)}.wait{color:var(--sf-warn,#d29922)}.dim{color:var(--sf-c-muted)}\
.chip{border:1px solid color-mix(in srgb,var(--sf-c-muted) 60%,transparent);border-radius:8px;padding:0 6px;font-size:11px;color:var(--sf-c-muted)}\
.files{width:34%;min-width:220px;max-width:460px;flex-shrink:0}\
.fl{gap:6px}.fk{width:14px;text-align:center;font-weight:700}.fk.A{color:var(--sf-good,#3fb950)}.fk.D{color:var(--sf-bad,#f85149)}.fk.R{color:var(--sf-warn,#d29922)}.fk.M{color:var(--sf-c-accent)}\
.fd{color:var(--sf-c-muted);overflow:hidden;text-overflow:ellipsis;flex:1}\
.fn{overflow:hidden;text-overflow:ellipsis}\
.cc{color:var(--sf-c-accent);font-size:11px}\
.who{font-weight:600}\
.head{font-family:Geist,system-ui,sans-serif;font-size:12.9px;padding:2px 4px;display:flex;gap:10px;align-items:center;white-space:nowrap;overflow:hidden}\
.title{font-weight:700;font-size:18px;overflow:hidden;text-overflow:ellipsis}\
.prompt{display:flex;gap:14px;align-items:center;height:28px;padding:0 10px;border:1px solid var(--sf-c-accent);border-radius:6px;font-family:Geist,system-ui,sans-serif;font-size:12.9px}\
.prompt .pl{color:var(--sf-c-accent);font-weight:600}\
.hsub{gap:8px;padding-top:4px;padding-bottom:6px}\
.br{font-family:\"Geist Mono\",ui-monospace,monospace;font-size:12px;padding:1px 6px;border-radius:4px;background:color-mix(in srgb,var(--sf-c-accent) 14%,transparent);color:var(--sf-c-accent)}\
.right{margin-left:auto}\
.acc{color:var(--sf-c-accent)}\
.ov{display:flex;gap:16px;height:100%;box-sizing:border-box;padding-top:8px;font-family:Geist,system-ui,sans-serif;font-size:12.9px;line-height:1.45;overflow:hidden}\
.ovmain{flex:1;min-width:0;display:flex;flex-direction:column;overflow-y:auto;padding-right:6px}\
.ovside{width:300px;flex-shrink:0;display:flex;flex-direction:column;overflow:hidden}\
.card{flex-shrink:0;margin-bottom:12px;border:1px solid color-mix(in srgb,var(--sf-c-muted) 32%,transparent);border-radius:8px;background:color-mix(in srgb,var(--sf-c-muted) 6%,transparent);padding:12px 16px}\
.card.cur{border-color:var(--sf-c-accent);background:color-mix(in srgb,var(--sf-c-accent) 7%,transparent)}\
.ch{font-weight:650;font-size:13.5px;margin-bottom:8px}\
.grh{color:var(--sf-c-muted);font-size:11.5px;font-weight:600;margin:8px 0 2px}\
.st{display:flex;align-items:center;gap:10px;padding:3px 0}\
.gl{width:16px;text-align:center;font-weight:700;flex-shrink:0}\
.ah{display:flex;align-items:baseline;gap:12px;flex-shrink:0;padding:4px 2px 10px}\
.at{font-weight:650;font-size:13.5px}\
.tli{display:flex;gap:12px;flex-shrink:0}\
.tlr{width:14px;flex-shrink:0;display:flex;flex-direction:column;align-items:center}\
.tld{width:10px;height:10px;border-radius:5px;margin-top:9px;flex-shrink:0;background:var(--sf-c-muted)}\
.tld.acc{background:var(--sf-c-accent)}.tld.ok{background:var(--sf-good,#3fb950)}.tld.wait{background:var(--sf-warn,#d29922)}.tld.bad{background:var(--sf-bad,#f85149)}\
.tls{flex:1;width:2px;margin-top:4px;background:color-mix(in srgb,var(--sf-c-muted) 28%,transparent)}\
.tlx{flex:1;min-width:0}\
.ev{display:flex;align-items:center;gap:8px;min-height:28px;margin-bottom:10px;padding:0 4px}\
.av{width:22px;height:22px;border-radius:11px;display:flex;align-items:center;justify-content:center;font-size:10px;font-weight:700;color:#fff;flex-shrink:0;line-height:1}\
.av0{background:#4f5bd5}.av1{background:#b4532a}.av2{background:#0f766e}.av3{background:#9d3fa8}.av4{background:#a16207}.av5{background:#2f6fd6}\
.name{font-weight:650;text-decoration:underline;text-decoration-color:color-mix(in srgb,var(--sf-c-accent) 60%,transparent)}\
.when{color:var(--sf-c-muted);font-size:11.5px;white-space:nowrap}\
.thd{display:flex;align-items:center;gap:10px;margin-bottom:10px;white-space:nowrap;overflow:hidden}\
.fname{font-weight:650}.fdir{overflow:hidden;text-overflow:ellipsis}.ln{color:var(--sf-c-accent);font-size:11.5px;white-space:nowrap}\
.chip.acc{border-color:var(--sf-c-accent);color:var(--sf-c-accent)}.chip.ok{border-color:var(--sf-good,#3fb950);color:var(--sf-good,#3fb950)}.chip.wait{border-color:var(--sf-warn,#d29922);color:var(--sf-warn,#d29922)}\
.snip{font-family:\"Geist Mono\",ui-monospace,monospace;font-size:12px;line-height:1.35;margin:0 0 4px;border-radius:6px;overflow:hidden;border:1px solid color-mix(in srgb,var(--sf-c-muted) 25%,transparent)}\
.cm{display:flex;gap:12px;padding:10px 0 4px}\
.cm.rep{border-top:1px solid color-mix(in srgb,var(--sf-c-muted) 22%,transparent)}\
.cmb{flex:1;min-width:0}\
.cmh{display:flex;align-items:center;gap:10px;margin-bottom:2px}\
.hint{color:var(--sf-c-muted);font-size:11.5px;padding-top:6px}\
.rv{display:flex;align-items:center;gap:10px;padding:4px 0}\
.vg{width:14px;text-align:center;font-weight:700;flex-shrink:0}\
.small{font-size:11.5px}\
.wi{padding:4px 0}.wk{color:var(--sf-c-accent);font-size:11.5px}\
.azfv{display:flex;gap:16px;height:100%;overflow:hidden;box-sizing:border-box;padding-top:10px;font-family:Geist,system-ui,sans-serif;font-size:12.9px}\
.aztree{max-height:100%;width:30%;min-width:240px;max-width:420px;flex-shrink:0;box-sizing:border-box;overflow-y:auto;padding:8px 6px;border:1px solid color-mix(in srgb,var(--sf-c-muted) 32%,transparent);border-radius:8px;background:color-mix(in srgb,var(--sf-c-muted) 4%,transparent)}\
.aztr{display:flex;align-items:center;gap:8px;height:26px;box-sizing:border-box;padding-right:10px;white-space:nowrap;border-radius:5px;cursor:pointer;overflow:hidden}\
.aztr:hover{background:color-mix(in srgb,var(--sf-c-muted) 14%,transparent)}\
.aztr.cur{background:color-mix(in srgb,var(--sf-c-accent) 24%,transparent)}\
.azdir{color:var(--sf-c-muted);font-weight:600}\
.azch{width:12px;text-align:center;flex-shrink:0}\
.azd0{padding-left:8px}.azd1{padding-left:24px}.azd2{padding-left:40px}.azd3{padding-left:56px}.azd4{padding-left:72px}.azd5{padding-left:88px}.azd6{padding-left:104px}.azd7{padding-left:120px}.azd8{padding-left:136px}.azd9{padding-left:152px}\
.azdp{max-height:100%;flex:1;min-width:0;display:flex;flex-direction:column;box-sizing:border-box;border:1px solid color-mix(in srgb,var(--sf-c-muted) 32%,transparent);border-radius:8px;overflow:hidden}\
.azdh{display:flex;align-items:center;gap:12px;padding:10px 14px;border-bottom:1px solid color-mix(in srgb,var(--sf-c-muted) 25%,transparent);white-space:nowrap;flex-shrink:0;overflow:hidden}\
.azds{flex:1;min-height:0;overflow-y:auto;padding:10px 12px 18px;font-family:\"Geist Mono\",ui-monospace,monospace;font-size:12px;line-height:1.35}\
.azic{margin:12px 8px 16px 44px;font-family:Geist,system-ui,sans-serif;font-size:12.9px;line-height:1.45}\
.azic .card{margin-bottom:0}\
.azmsg{padding:16px;color:var(--sf-c-muted)}\
.azseg{display:flex;margin:0 0 6px;border-radius:6px;overflow:hidden;background:color-mix(in srgb,var(--sf-c-muted) 6%,transparent)}\
.azside{flex:1 1 0;min-width:0;display:flex;padding:4px 0}\
.azseg.split>.azside:first-child{box-shadow:inset -1px 0 0 color-mix(in srgb,var(--sf-c-muted) 28%,transparent)}\
.azgc{flex:none;user-select:none;-webkit-user-select:none}\
.azgl{position:relative;height:18px;line-height:18px;box-sizing:border-box;padding:0 8px 0 24px;white-space:pre;color:var(--sf-c-muted);font-variant-numeric:tabular-nums;cursor:pointer}\
.azgl.add{background:color-mix(in srgb,var(--sf-good,#3fb950) 16%,transparent)}\
.azgl.del{background:color-mix(in srgb,var(--sf-bad,#f85149) 16%,transparent)}\
.azgl.nil{cursor:default;background:color-mix(in srgb,var(--sf-c-muted) 9%,transparent)}\
.azgl.gap{cursor:default;text-align:center;padding:0 8px}\
.azgl:not(.nil):not(.gap):hover{color:var(--sf-c-accent)}\
.azgl:not(.nil):not(.gap):hover::before{content:'+';position:absolute;left:4px;top:2px;width:14px;height:14px;line-height:14px;text-align:center;border-radius:3px;background:var(--sf-c-accent);color:#fff;font-weight:700}\
.azcc{flex:1;min-width:0}\
.azcc .sf-code{--sf-lh:18px;line-height:18px;font-size:12px;margin:0;border-radius:0;background:none;box-shadow:none}\
.azcc .sf-code-body{padding:0}\
.azcbox{margin:8px 8px 14px 44px;padding:12px 14px;border:1px solid color-mix(in srgb,var(--sf-c-accent) 45%,transparent);border-radius:8px;display:flex;flex-direction:column;gap:10px;font-family:Geist,system-ui,sans-serif;font-size:12.9px}\
.azcbh{display:flex;align-items:center;gap:10px}\
.azcbf{display:flex;justify-content:flex-end;gap:8px}\
.azbtn{padding:4px 14px;border-radius:6px;border:1px solid color-mix(in srgb,var(--sf-c-muted) 45%,transparent);background:none;color:inherit;font:inherit;cursor:pointer}\
.azbtn.pri{background:var(--sf-c-accent);border-color:var(--sf-c-accent);color:#fff}\
.aztog{display:flex;flex-shrink:0;border:1px solid color-mix(in srgb,var(--sf-c-muted) 45%,transparent);border-radius:6px;overflow:hidden}\
.aztog span{padding:2px 10px;cursor:pointer;color:var(--sf-c-muted)}\
.aztog span.on{background:color-mix(in srgb,var(--sf-c-accent) 22%,transparent);color:var(--sf-c-accent)}\
.strip{height:100%;box-sizing:border-box;display:flex;flex-direction:column;overflow:hidden;padding-top:10px;font-family:Geist,system-ui,sans-serif;font-size:12.9px;line-height:1.45}";

#[derive(Clone, Debug)]
enum Msg {
	/// A list row was double-clicked.
	Open(u32),
	/// A top bar dropdown was clicked: open it, or close it if it is open.
	Menu(Menu),
	/// The project filter in the top bar (None: every project).
	Project(Option<String>),
	/// The status filter in the top bar.
	Status(String),
	/// The sort order in the top bar.
	Sort(String),
	ListTab(String),
	PrTab(String),
	File(usize),
	/// A folder row in the file tree: fold or unfold it.
	Folder(String),
	/// Side by side (true) or inline.
	Split(bool),
	/// The whole file (true) or the changes with context.
	Full(bool),
	Thread(u32),
	/// A diff line number was clicked: comment on that line (the old side when true).
	LineAt(u32, bool),
	/// The line comment box's buttons.
	Cancel,
	Submit,
	/// The next (true) or previous change in the diff.
	Change(bool),
	/// Start an AI agent on the PR, or on a comment thread, setting the repo up first when it has no checkout yet.
	Ai(review::Kind, Option<u32>),
	/// The gear: show or hide the review settings menu.
	ReviewMenu,
	/// The menu: edit one kind of prompt.
	EditPrompt(review::Kind),
	/// The menu: pick this repo's checkout.
	ReviewSetup,
	/// The menu: this repo's agent.
	ReviewAgent,
	/// The comment box on a diff line: ask an AI about that line.
	AiLine(review::Kind),
	/// The checkout picker: show this folder.
	PickGo(std::path::PathBuf),
	/// The checkout picker: use the folder shown.
	PickUse,
	/// The checkout picker: cancel.
	PickCancel,
	/// Show or hide the prompt preview.
	Preview,
	Send(String),
	/// Native editing changed the composer's text.
	Edit(wire::Edit),
	Focus,
}

/// How the rail orders the PRs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SortMode {
	/// Grouped by project, then repo, newest first within each.
	Project,
	/// Every project together, most recently created first.
	Newest,
	/// Every project together, oldest first.
	Oldest,
}

impl SortMode {
	const ALL: [Self; 3] = [Self::Project, Self::Newest, Self::Oldest];

	fn id(self) -> &'static str {
		match self {
			Self::Project => "project",
			Self::Newest => "newest",
			Self::Oldest => "oldest",
		}
	}

	fn label(self) -> &'static str {
		match self {
			Self::Project => "By project",
			Self::Newest => "Newest first",
			Self::Oldest => "Oldest first",
		}
	}
}

/// The dropdowns in the top bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Menu {
	Show,
	Project,
	Status,
	Sort,
}

/// The status filter in the top bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StatusFilter {
	Any,
	Ready,
	Draft,
	Approved,
	Blocked,
}

impl StatusFilter {
	const ALL: [Self; 5] = [Self::Any, Self::Ready, Self::Draft, Self::Approved, Self::Blocked];

	fn id(self) -> &'static str {
		match self {
			Self::Any => "any",
			Self::Ready => "ready",
			Self::Draft => "draft",
			Self::Approved => "approved",
			Self::Blocked => "blocked",
		}
	}

	fn label(self) -> &'static str {
		match self {
			Self::Any => "Any status",
			Self::Ready => "Ready",
			Self::Draft => "Draft",
			Self::Approved => "Approved",
			Self::Blocked => "Blocked",
		}
	}

	fn matches(self, p: &Pr) -> bool {
		let blocked = p.reviewers.iter().any(|r| r.vote < 0) || matches!(p.merge_status.as_deref(), Some("conflicts" | "failure"));
		match self {
			Self::Any => true,
			Self::Ready => !p.is_draft,
			Self::Draft => p.is_draft,
			Self::Approved => p.reviewers.iter().any(|r| r.vote >= 5) && !blocked,
			Self::Blocked => blocked,
		}
	}
}

/// "Zach Christmas" becomes "Zach C.".
fn short_name(name: &str) -> String {
	let mut w = name.split_whitespace();
	match (w.next(), w.last()) {
		(Some(f), Some(l)) => format!("{f} {}.", l.chars().next().unwrap_or_default()),
		(Some(f), None) => f.to_owned(),
		_ => String::new(),
	}
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ListTab {
	Review,
	Mine,
	All,
}

impl ListTab {
	const ALL: [Self; 3] = [Self::Review, Self::Mine, Self::All];

	fn id(self) -> &'static str {
		match self {
			Self::Review => "review",
			Self::Mine => "mine",
			Self::All => "all",
		}
	}

	fn label(self) -> &'static str {
		match self {
			Self::Review => "Reviewing",
			Self::Mine => "Created by me",
			Self::All => "All active",
		}
	}
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PrTab {
	Overview,
	Files,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ComposeKind {
	/// The organization URL, on first run.
	Org,
	Filter,
	/// A new thread when none, else a reply in that thread.
	Comment(Option<u32>),
	/// The quick switcher: type to pick any pull request.
	Jump,
	/// Which agent to run.
	ReviewAgent,
	/// Edit one kind of prompt.
	ReviewPrompt(review::Kind),
	/// The line an inline comment goes on.
	Line,
	/// An inline comment on a line of the selected file (the old side when `left`).
	Inline { line: u32, left: bool },
}

struct Composer {
	kind:   ComposeKind,
	draft:  String,
	/// In chars.
	cursor: usize,
}

impl Composer {
	fn new(kind: ComposeKind, draft: String) -> Self {
		let cursor = draft.chars().count();
		Self { kind, draft, cursor }
	}

	fn byte(&self, chars: usize) -> usize {
		self.draft.char_indices().nth(chars).map_or(self.draft.len(), |(i, _)| i)
	}

	fn insert(&mut self, text: &str) {
		let at = self.byte(self.cursor);
		self.draft.insert_str(at, text);
		self.cursor += text.chars().count();
	}

	/// Plain editing keys. False when the key is not one of them.
	fn key(&mut self, key: &Key) -> bool {
		match key.name.as_str() {
			"backspace" if self.cursor > 0 => {
				let at = self.byte(self.cursor - 1);
				self.draft.remove(at);
				self.cursor -= 1;
			},
			"delete" if self.cursor < self.draft.chars().count() => {
				let at = self.byte(self.cursor);
				self.draft.remove(at);
			},
			"backspace" | "delete" => {},
			"left" => self.cursor = self.cursor.saturating_sub(1),
			"right" => self.cursor = (self.cursor + 1).min(self.draft.chars().count()),
			"home" => self.cursor = 0,
			"end" => self.cursor = self.draft.chars().count(),
			"paste" => self.insert(&key.text.clone().unwrap_or_default().replace("\r\n", "\n")),
			_ => match key.typed() {
				Some(t) if !key.ctrl => {
					let t = t.to_owned();
					self.insert(&t);
				},
				_ => return false,
			},
		}
		true
	}

	/// The char index at UTF-16 offset `u`.
	fn char_at(&self, u: usize) -> usize {
		let mut units = 0;
		for (i, c) in self.draft.chars().enumerate() {
			if units >= u {
				return i;
			}
			units += c.len_utf16();
		}
		self.draft.chars().count()
	}

	/// Applies a native edit. False when Tern's copy of the text has a different length.
	fn apply(&mut self, e: &wire::Edit) -> bool {
		if e.len != self.draft.encode_utf16().count() {
			return false;
		}
		let (a, b) = (self.byte(self.char_at(e.from.min(e.to))), self.byte(self.char_at(e.from.max(e.to))));
		self.draft.replace_range(a..b, &e.text);
		self.cursor = self.char_at(e.cursor);
		true
	}

	fn utf16_cursor(&self) -> usize {
		self.draft.chars().take(self.cursor).map(char::len_utf16).sum()
	}
}

/// One file's loaded sides and its built diff.
enum FileState {
	Loading,
	Failed(String),
	Ready { sides: Sides, diff: FileDiff },
}

/// One AI request: what to ask, about which comment thread, or which diff line (its number, whether it is on the old side, and the
/// comment being drafted).
#[derive(Clone, Debug)]
struct AiReq {
	kind:   review::Kind,
	thread: Option<u32>,
	line:   Option<(u32, bool, String)>,
}

/// Where an AI question or setting is asked: by the buttons at the top, by a comment, or in a line's comment box.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AiAnchor {
	Header,
	Thread(u32),
	Line,
}

impl AiReq {
	fn anchor(&self) -> AiAnchor {
		if self.line.is_some() {
			return AiAnchor::Line;
		}
		match (self.thread, self.kind) {
			(Some(t), review::Kind::Explain | review::Kind::Respond) => AiAnchor::Thread(t),
			_ => AiAnchor::Header,
		}
	}
}

struct Page {
	pr:      Pr,
	tab:     PrTab,
	detail:  Option<Detail>,
	error:   Option<String>,
	file:    usize,
	files:   HashMap<String, FileState>,
	/// Diffs side by side.
	split:   bool,
	/// Folders folded in the file tree.
	folded:  HashSet<String>,
	/// Whole files instead of hunks.
	full:    bool,
	/// The selected comment thread.
	thread:  Option<u32>,
	/// A thread to scroll the diff to once the selected file loads.
	pending: Option<u32>,
	/// The change the last jump went to (its place in the file's list of changes).
	change:  Option<usize>,
	/// A jump to make once the selected file loads: its first change (true) or last (false).
	go:      Option<bool>,
}

impl Page {
	fn new(pr: Pr) -> Self {
		Self {
			pr,
			tab: PrTab::Overview,
			detail: None,
			error: None,
			file: 0,
			files: HashMap::new(),
			split: false,
			folded: HashSet::new(),
			full: false,
			thread: None,
			pending: None,
			change: None,
			go: None,
		}
	}

	fn change(&self) -> Option<&api::Change> {
		self.detail.as_ref()?.changes.get(self.file)
	}

	/// Threads on the selected file, by line.
	fn file_threads(&self) -> Vec<&Thread> {
		let (Some(d), Some(c)) = (self.detail.as_ref(), self.change()) else { return Vec::new() };
		let mut t: Vec<&Thread> = d.threads.iter().filter(|t| t.shown() && t.file() == Some(c.path())).collect();
		t.sort_by_key(|t| t.line().unwrap_or(0));
		t
	}

	/// Everything that happened, newest first: comment threads (by their latest comment), system events and the creation.
	fn timeline(&self) -> Vec<Item<'_>> {
		let mut items: Vec<(i64, Item<'_>)> = Vec::new();
		if let Some(d) = &self.detail {
			for t in &d.threads {
				if t.shown() {
					items.push((stamp(t.latest()), Item::Thread(t)));
				} else if let Some(e) = t.event() {
					items.push((stamp(t.started()), Item::Event(e, t.started())));
				}
			}
		}
		items.push((stamp(&self.pr.creation_date), Item::Created));
		items.sort_by_key(|i| std::cmp::Reverse(i.0));
		items.into_iter().map(|(_, i)| i).collect()
	}

	/// Comment threads in timeline order.
	fn all_threads(&self) -> Vec<&Thread> {
		self.timeline().into_iter().filter_map(|i| if let Item::Thread(t) = i { Some(t) } else { None }).collect()
	}

	/// The code a thread is about (its first five lines at most), with two lines before it: from the diff when those lines changed, else from the file.
	fn snippet(&self, t: &Thread) -> Option<Vec<Hunk>> {
		let FileState::Ready { sides, diff } = self.files.get(t.file()?)? else { return None };
		let line = u64::from(t.line()?);
		let end = u64::from(t.line_end().unwrap_or(0)).clamp(line, line + 4);
		let left = t.on_left();
		let at = diff.lines.iter().position(|l| if left { l.old == Some(line) && l.tag != '+' } else { l.new == Some(line) && l.tag != '-' });
		if let Some(i) = at {
			let start = i.saturating_sub(2);
			return Some(diff.window(start, i - start + (end - line) as usize + 1));
		}
		let text = if left { sides.0.as_ref()? } else { sides.1.as_ref()? };
		let lines: Vec<&str> = text.lines().collect();
		let first = line.saturating_sub(2).max(1);
		let last = end.min(lines.len() as u64);
		if first > last {
			return None;
		}
		let body = (first..=last).map(|n| format!(" {}", lines[n as usize - 1])).collect();
		Some(vec![Hunk { old_start: Some(first), new_start: Some(first), lines: body }])
	}

	/// The selected file has line `n` on its new side (old side when `left`).
	fn has_line(&self, n: u32, left: bool) -> bool {
		let Some(c) = self.change() else { return false };
		match self.files.get(c.path()) {
			Some(FileState::Ready { sides, .. }) => {
				let side = if left { &sides.0 } else { &sides.1 };
				n > 0 && side.as_ref().is_some_and(|t| t.lines().count() >= n as usize)
			},
			_ => false,
		}
	}

	/// The change paths, in change-list order.
	fn paths(&self) -> Vec<&str> {
		self.detail.as_ref().map(|d| d.changes.iter().map(api::Change::path).collect()).unwrap_or_default()
	}

	/// Whether the selected file's diff is on screen (so its thread cards exist).
	fn file_ready(&self) -> bool {
		self.change().is_some_and(|c| matches!(self.files.get(c.path()), Some(FileState::Ready { .. })))
	}

	/// The selected file's diff, when it has lines to show.
	fn diff(&self) -> Option<&FileDiff> {
		match self.files.get(self.change()?.path())? {
			FileState::Ready { diff, .. } if !diff.is_empty() => Some(diff),
			_ => None,
		}
	}

}

struct App {
	client:  Option<Arc<Client>>,
	me:      Option<Identity>,
	prs:     Vec<Pr>,
	loading: bool,
	error:   Option<String>,
	tab:     ListTab,
	filter:  String,
	sel:     Option<u32>,
	/// The project filter (None: every project).
	project: Option<String>,
	/// The open dropdown in the top bar.
	menu:    Option<Menu>,
	status:  StatusFilter,
	sort:    SortMode,
	/// The highlighted result in the switcher.
	jump:    usize,
	/// The review settings being edited, the key of the repo they belong to, and whether to start the agent when saved.
	review:  config::Review,
	review_key: String,
	/// What to run once the repo's checkout and agent are set (a kind, and the comment thread for comment prompts).
	review_next: Option<AiReq>,
	/// The folder the checkout picker shows, and the highlighted row.
	pick: Option<std::path::PathBuf>,
	pick_sel: usize,
	/// What is typed to search the picker's folders.
	pick_q: String,
	/// Where the picker or prompt editor shows.
	ai_at: AiAnchor,
	/// The review settings menu behind the gear is open.
	rv_menu: bool,
	/// Show the prompt being edited with its variables filled in.
	preview: bool,
	page:    Option<Page>,
	compose: Option<Composer>,
	voting:  bool,
	note:    String,
	rows:    f64,
	cell_h:  f64,
	cols:    f64,
	tx:      Sender<Reply>,
	rx:      Receiver<Reply>,
	busy:    usize,
	/// A scroll for Tern to do after the next render.
	scroll_req: Option<ScrollReq>,
	/// A file tree row to bring into view after the next render.
	tree_req:   Option<String>,
	/// A `g` was pressed: a second one goes to the top.
	g_pending:  bool,
}

/// A vim motion.
#[derive(Clone, Copy)]
enum Motion {
	Top,
	Bottom,
	/// A share of a screen, down when positive.
	Screen(f64),
}

enum ScrollReq {
	By(ScrollBy),
	/// Lines down (up when negative).
	Lines(i64),
	/// Bring a node to the top of the overview.
	Reveal(String),
}

/// The overview's scroll container, and a node inside it (for `scroll`, which acts on a node's container).
const OV_MAIN: &str = "main.pbody.ov.ovm";
const OV_FIRST: &str = "main.pbody.ov.ovm.status";
/// The Files tab's file tree, its diff's scroll container, and the node at the diff's top.
const TREE: &str = "main.pbody.fv.tree";
const FILES_DS: &str = "main.pbody.fv.dp.ds";
const FILES_FIRST: &str = "main.pbody.fv.dp.ds.top";

/// A folder row's key: its path hashed, because node ids are dotted and paths can hold dots.
fn dir_key(path: &str) -> String {
	let h = path.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
	format!("d{h:x}")
}

/// A line of the overview's timeline.
enum Item<'a> {
	Thread(&'a Thread),
	/// A system event and when it happened.
	Event(api::Event, &'a str),
	Created,
}

/// Milliseconds since the epoch for an RFC 3339 time (0 when it does not parse).
fn stamp(when: &str) -> i64 {
	chrono::DateTime::parse_from_rfc3339(when).map_or(0, |t| t.timestamp_millis())
}

/// `just now`, `12m ago`, `5h ago`, `yesterday`, `3d ago`, then the date.
fn ago(when: &str) -> String {
	let Ok(t) = chrono::DateTime::parse_from_rfc3339(when) else { return String::new() };
	let secs = (chrono::Utc::now() - t.with_timezone(&chrono::Utc)).num_seconds().max(0);
	match secs {
		s if s < 60 => "just now".into(),
		s if s < 3600 => format!("{}m ago", s / 60),
		s if s < 86_400 => format!("{}h ago", s / 3600),
		s if s < 86_400 * 2 => "yesterday".into(),
		s if s < 86_400 * 30 => format!("{}d ago", s / 86_400),
		_ => t.with_timezone(&chrono::Local).format("%b %-d, %Y").to_string(),
	}
}

/// The full local time, shown on hover.
fn when_full(when: &str) -> String {
	chrono::DateTime::parse_from_rfc3339(when).map(|t| t.with_timezone(&chrono::Local).format("%a %b %-d, %Y at %-I:%M %p").to_string()).unwrap_or_default()
}

/// `PS` for Priya Shah.
/// Lines `from..to` of a diff: clickable line numbers beside highlighted code, side by side when `split`.
fn segment(d: &FileDiff, from: usize, to: usize, split: bool, lang: &str) -> tern_sdk::Node<Msg> {
	let (mut left, mut right) = diff::cells(&d.lines[from..to], split);
	// A segment that starts at a new hunk keeps the gap mark the cut took away.
	if from > 0 && d.lines[from].hunk != d.lines[from - 1].hunk {
		left.insert(0, diff::Cell::Gap);
		if split {
			right.insert(0, diff::Cell::Gap);
		}
	}
	let seg = html::div().key(format!("s{from}")).class(if split { "azseg split" } else { "azseg" });
	if split {
		seg.child(side(&left, Some(true), lang)).child(side(&right, Some(false), lang)).into()
	} else {
		seg.child(side(&left, None, lang)).into()
	}
}

/// One column of diff rows: `Some(true)` the old side, `Some(false)` the new, `None` both in one.
fn side(cells: &[diff::Cell<'_>], old: Option<bool>, lang: &str) -> html::El<Msg> {
	let mut gutter: Vec<tern_sdk::Node<Msg>> = Vec::with_capacity(cells.len());
	let mut text = String::new();
	let mut marks = Vec::new();
	for (i, cell) in cells.iter().enumerate() {
		if i > 0 {
			text.push('\n');
		}
		let row = match cell {
			diff::Cell::Nil => html::div().class("azgl nil"),
			diff::Cell::Gap => html::div().class("azgl gap").text("⋯"),
			diff::Cell::Line(l) => {
				text.push_str(&l.text);
				let tone = match l.tag {
					'+' => Some(ui::Tone::Success),
					'-' => Some(ui::Tone::Error),
					_ => None,
				};
				if tone.is_some() {
					marks.push(ui::CodeMark { line: i as u64 + 1, tone, ranges: Vec::new() });
				}
				let num = |n: Option<u64>| n.map_or_else(|| "    ".to_owned(), |n| format!("{n:>4}"));
				let (label, at) = match old {
					None => (format!("{} {} {}", num(l.old), num(l.new), l.tag), if l.tag == '-' { (l.old, true) } else { (l.new, false) }),
					Some(true) => (format!("{} {}", num(l.old), l.tag), (l.old, true)),
					Some(false) => (format!("{} {}", num(l.new), l.tag), (l.new, false)),
				};
				let cls = match l.tag {
					'+' => "azgl add",
					'-' => "azgl del",
					_ => "azgl",
				};
				let row = html::div().class(cls).text(label);
				match at {
					(Some(n), left) => row.title(format!("Comment on line {n}")).on_click(Msg::LineAt(u32::try_from(n).unwrap_or(u32::MAX), left)),
					(None, _) => row,
				}
			},
		};
		gutter.push(row.into());
	}
	let mut code = ui::code(text).marks(marks);
	if !lang.is_empty() {
		code = code.lang(lang.to_owned());
	}
	html::div()
		.class("azside")
		.child(html::div().class("azgc").children(gutter))
		.child(html::div().class("azcc").child(code))
}

fn initials(name: &str) -> String {
	let mut words = name.split_whitespace().filter_map(|w| w.chars().find(|c| c.is_alphanumeric()));
	let first = words.next();
	let last = words.next_back();
	[first, last].into_iter().flatten().flat_map(char::to_uppercase).collect()
}

/// A round badge with someone's initials, the same color for the same name everywhere.
fn avatar(name: &str) -> html::El<Msg> {
	let h = name.bytes().fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(u32::from(b)));
	html::span(initials(name)).class(format!("av av{}", h % 6)).title(name.to_owned())
}

fn person(name: &str) -> html::El<Msg> {
	html::span(name.to_owned()).class("name")
}

fn time(when: &str, extra: &str) -> html::El<Msg> {
	html::span(ago(when)).class(format!("when {extra}")).title(when_full(when))
}

/// A timeline entry: a dot and the line down to the next entry, then the entry.
fn tl_item(dot: &str, last: bool, body: html::El<Msg>) -> html::El<Msg> {
	let mut rail = html::div().class("tlr").child(html::div().class(format!("tld {dot}")));
	if !last {
		rail = rail.child(html::div().class("tls"));
	}
	html::div().class("tli").child(rail).child(html::div().class("tlx").child(body))
}

/// One line of the timeline: who did what, and when.
fn event_row(who: &str, what: &str, when: &str) -> html::El<Msg> {
	html::div().class("ev").child(avatar(who)).child(person(who)).child(html::span(what.to_owned())).child(time(when, "right"))
}

/// `3d`, `5h`, `12m` since an RFC 3339 time.
fn age(when: &str) -> String {
	let Ok(t) = chrono::DateTime::parse_from_rfc3339(when) else { return String::new() };
	let secs = (chrono::Utc::now() - t.with_timezone(&chrono::Utc)).num_seconds().max(0);
	match secs {
		s if s < 3600 => format!("{}m", s / 60),
		s if s < 86_400 => format!("{}h", s / 3600),
		s if s < 86_400 * 60 => format!("{}d", s / 86_400),
		s => format!("{}mo", s / (86_400 * 30)),
	}
}

fn vote_glyph(vote: i32) -> (&'static str, &'static str) {
	match vote {
		10 => ("✓", "ok"),
		5 => ("✓", "ok"),
		-5 => ("⏸", "wait"),
		-10 => ("✗", "bad"),
		_ => ("○", "dim"),
	}
}

/// The first line of a comment, cut to `n` chars.
fn first_line(s: &str, n: usize) -> String {
	let line = s.lines().find(|l| !l.trim().is_empty()).unwrap_or_default().trim();
	if line.chars().count() > n { format!("{}…", line.chars().take(n).collect::<String>()) } else { line.to_owned() }
}

fn open_url(url: &str) -> anyhow::Result<()> {
	#[cfg(windows)]
	let mut cmd = {
		let mut c = std::process::Command::new("rundll32");
		c.args(["url.dll,FileProtocolHandler", url]);
		c
	};
	#[cfg(target_os = "macos")]
	let mut cmd = {
		let mut c = std::process::Command::new("open");
		c.arg(url);
		c
	};
	#[cfg(all(unix, not(target_os = "macos")))]
	let mut cmd = {
		let mut c = std::process::Command::new("xdg-open");
		c.arg(url);
		c
	};
	cmd.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn()?;
	Ok(())
}

/// The pane's height in lines, from `tern inspect` (the surface is not told when it changes).
fn pane_rows() -> Option<f64> {
	let id: u64 = std::env::var("TERN_PANE").ok()?.parse().ok()?;
	let out = std::process::Command::new("tern").args(["inspect", "--json"]).output().ok()?;
	let v: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
	let mut stack = vec![&v];
	while let Some(n) = stack.pop() {
		match n {
			serde_json::Value::Object(m) => {
				if m.get("id").and_then(serde_json::Value::as_u64) == Some(id)
					&& let Some(r) = m.get("rows").and_then(serde_json::Value::as_f64)
				{
					return Some(r);
				}
				stack.extend(m.values());
			},
			serde_json::Value::Array(a) => stack.extend(a),
			_ => {},
		}
	}
	None
}

impl App {
	fn new(org: Option<String>, demo: bool) -> Self {
		let (tx, rx) = mpsc::channel();
		let mut app = Self {
			client: None,
			me: None,
			prs: Vec::new(),
			loading: false,
			error: None,
			tab: ListTab::Review,
			filter: String::new(),
			sel: None,
			project: None,
			menu: None,
			status: StatusFilter::Any,
			sort: SortMode::Project,
			jump: 0,
			review: config::Review::default(),
			review_key: String::new(),
			review_next: None,
			pick: None,
			pick_sel: 0,
			pick_q: String::new(),
			ai_at: AiAnchor::Header,
			rv_menu: false,
			preview: false,
			page: None,
			compose: None,
			voting: false,
			note: String::new(),
			rows: pane_rows().unwrap_or(40.0),
			cell_h: 20.0,
			cols: 160.0,
			tx,
			rx,
			busy: 0,
			scroll_req: None,
			g_pending: false,
			tree_req: None,
		};
		if demo {
			app.client = Some(Arc::new(Client::demo()));
			app.refresh();
		} else {
			match org {
				Some(o) => app.connect(&o),
				None => app.compose = Some(Composer::new(ComposeKind::Org, String::new())),
			}
		}
		app
	}

	fn connect(&mut self, org: &str) {
		self.client = Some(Arc::new(Client::new(org)));
		self.refresh();
	}

	fn start(&mut self, job: Job) {
		if let Some(c) = &self.client {
			self.busy += 1;
			worker::start(c.clone(), job, self.tx.clone());
		}
	}

	fn refresh(&mut self) {
		self.loading = true;
		self.start(Job::Load);
		if let Some(p) = &self.page {
			let pr = p.pr.clone();
			self.start(Job::Detail(pr));
		}
	}

	fn me_id(&self) -> &str {
		self.me.as_ref().map_or("", |m| m.id.as_str())
	}

	fn in_tab(&self, pr: &Pr, tab: ListTab) -> bool {
		let me = self.me_id();
		match tab {
			ListTab::Review => pr.reviewers.iter().any(|r| r.id == me) && pr.created_by.id != me,
			ListTab::Mine => pr.created_by.id == me,
			ListTab::All => true,
		}
	}

	/// The current tab's PRs after the filter, by project, repo, then newest first.
	fn visible(&self) -> Vec<&Pr> {
		let needle = self.filter.to_lowercase();
		let mut v: Vec<&Pr> = self
			.prs
			.iter()
			.filter(|p| self.in_tab(p, self.tab))
			.filter(|p| self.status.matches(p))
			.filter(|p| self.project.as_ref().is_none_or(|n| *n == p.repository.project.name))
			.filter(|p| {
				needle.is_empty()
					|| [p.title.as_str(), p.created_by.display_name.as_str(), p.repository.name.as_str(), p.repository.project.name.as_str(), p.source()]
						.iter()
						.any(|s| s.to_lowercase().contains(&needle))
					|| p.pull_request_id.to_string() == needle.trim_start_matches('!')
			})
			.collect();
		match self.sort {
			SortMode::Project => v.sort_by(|a, b| {
				(a.repository.project.name.to_lowercase(), a.repository.name.to_lowercase())
					.cmp(&(b.repository.project.name.to_lowercase(), b.repository.name.to_lowercase()))
					.then(b.pull_request_id.cmp(&a.pull_request_id))
			}),
			SortMode::Newest => v.sort_by(|a, b| stamp(&b.creation_date).cmp(&stamp(&a.creation_date)).then(b.pull_request_id.cmp(&a.pull_request_id))),
			SortMode::Oldest => v.sort_by(|a, b| stamp(&a.creation_date).cmp(&stamp(&b.creation_date)).then(a.pull_request_id.cmp(&b.pull_request_id))),
		}
		v
	}

	fn selected_pr(&self) -> Option<&Pr> {
		if let Some(p) = &self.page {
			return Some(&p.pr);
		}
		let id = self.sel?;
		self.prs.iter().find(|p| p.pull_request_id == id)
	}

	fn ids(&self) -> Vec<u32> {
		self.visible().iter().map(|p| p.pull_request_id).collect()
	}

	fn fix_selection(&mut self) {
		// A project filter that no longer matches anything (after a tab or status change) drops away.
		if let Some(n) = self.project.clone()
			&& !self.prs.iter().any(|p| self.in_tab(p, self.tab) && self.status.matches(p) && p.repository.project.name == n)
		{
			self.project = None;
		}
		let ids = self.ids();
		if !self.sel.is_some_and(|s| ids.contains(&s)) {
			self.sel = ids.first().copied();
		}
	}

	fn move_sel(&mut self, delta: i64) {
		let ids = self.ids();
		if ids.is_empty() {
			return;
		}
		let at = self.sel.and_then(|s| ids.iter().position(|i| *i == s)).unwrap_or(0) as i64;
		self.sel = Some(ids[(at + delta).clamp(0, ids.len() as i64 - 1) as usize]);
	}

	/// The open PR's siblings in its project, in list order.
	fn siblings(&self) -> Vec<u32> {
		let Some(p) = &self.page else { return Vec::new() };
		let proj = &p.pr.repository.project.name;
		self.visible().iter().filter(|q| q.repository.project.name == *proj).map(|q| q.pull_request_id).collect()
	}

	/// Open the previous or next PR in the same project.
	fn step_pr(&mut self, forward: bool) {
		let ids = self.siblings();
		let Some(at) = self.page.as_ref().and_then(|p| ids.iter().position(|i| *i == p.pr.pull_request_id)) else { return };
		let to = if forward { at + 1 } else { at.wrapping_sub(1) };
		if let Some(id) = ids.get(to) {
			self.open(*id);
		}
	}

	/// Every active PR matching each word of `q`, by project, repo, then newest first.
	fn jump_results(&self, q: &str) -> Vec<u32> {
		let words: Vec<String> = q.to_lowercase().split_whitespace().map(|w| w.trim_start_matches('!').to_owned()).collect();
		let mut v: Vec<&Pr> = self
			.prs
			.iter()
			.filter(|p| {
				let hay = format!("{} {} {} {} {} {}", p.pull_request_id, p.title, p.created_by.display_name, p.repository.name, p.repository.project.name, p.source()).to_lowercase();
				words.iter().all(|w| hay.contains(w.as_str()))
			})
			.collect();
		v.sort_by(|a, b| {
			(a.repository.project.name.to_lowercase(), a.repository.name.to_lowercase())
				.cmp(&(b.repository.project.name.to_lowercase(), b.repository.name.to_lowercase()))
				.then(b.pull_request_id.cmp(&a.pull_request_id))
		});
		v.iter().map(|p| p.pull_request_id).collect()
	}

	fn open(&mut self, id: u32) {
		let Some(pr) = self.prs.iter().find(|p| p.pull_request_id == id).cloned() else { return };
		self.menu = None;
		if self.compose.as_ref().is_some_and(|c| c.kind == ComposeKind::Jump) {
			self.compose = None;
		}
		self.sel = Some(id);
		self.page = Some(Page::new(pr.clone()));
		self.start(Job::Detail(pr));
	}

	/// Loads the selected file's sides if they are not loaded yet.
	fn load_file(&mut self) {
		if let Some(i) = self.page.as_ref().map(|p| p.file) {
			self.load_change(i);
		}
	}

	/// Loads every file that has comments, so the overview can show the code each one is about.
	fn prefetch_threads(&mut self) {
		let Some(d) = self.page.as_ref().and_then(|p| p.detail.as_ref()) else { return };
		let want: Vec<usize> =
			d.changes.iter().enumerate().filter(|(_, c)| d.threads.iter().any(|t| t.shown() && t.file() == Some(c.path()))).map(|(i, _)| i).collect();
		for i in want {
			self.load_change(i);
		}
	}

	/// Loads change `i`'s sides if they are not loaded yet.
	fn load_change(&mut self, i: usize) {
		let Some(p) = &self.page else { return };
		let Some(c) = p.detail.as_ref().and_then(|d| d.changes.get(i)) else { return };
		let path = c.path().to_owned();
		if p.files.contains_key(&path) {
			return;
		}
		let (base, head) = c.blobs();
		let job = Job::File { pr: p.pr.clone(), path: path.clone(), base: base.map(str::to_owned), head: head.map(str::to_owned) };
		if let Some(p) = &mut self.page {
			p.files.insert(path, FileState::Loading);
		}
		self.start(job);
	}

	fn rebuild_diffs(&mut self) {
		if let Some(p) = &mut self.page {
			let context = if p.full { usize::MAX / 4 } else { CONTEXT };
			for state in p.files.values_mut() {
				if let FileState::Ready { sides: (Some(a), Some(b)), diff } = state {
					*diff = diff::build(a, b, context);
				}
			}
			p.change = None;
			// The whole file opens at its first change, the changes at the top.
			if p.full && p.file_ready() {
				p.go = Some(true);
			} else {
				self.scroll_req = Some(ScrollReq::By(ScrollBy::Start));
			}
		}
		self.finish_go();
	}

	/// Handles finished jobs. True when something changed.
	fn poll(&mut self) -> bool {
		let mut changed = false;
		while let Ok(reply) = self.rx.try_recv() {
			changed = true;
			self.busy = self.busy.saturating_sub(1);
			match reply {
				Reply::Loaded(Ok((me, prs))) => {
					let first = self.me.is_none();
					self.me = Some(me);
					self.prs = prs;
					self.loading = false;
					self.error = None;
					if first && !self.prs.iter().any(|p| self.in_tab(p, ListTab::Review)) {
						self.tab = if self.prs.iter().any(|p| self.in_tab(p, ListTab::Mine)) { ListTab::Mine } else { ListTab::All };
					}
					self.fix_selection();
					if self.page.is_none()
						&& self.cols >= RAIL_MIN_COLS
						&& let Some(id) = self.sel
					{
						self.open(id);
					}
				},
				Reply::Loaded(Err(e)) => {
					self.loading = false;
					self.error = Some(e);
				},
				Reply::Detail(id, r) => {
					let Some(p) = self.page.as_mut().filter(|p| p.pr.pull_request_id == id) else { continue };
					match r {
						Ok(b) => {
							let (pr, detail) = *b;
							if let Some(slot) = self.prs.iter_mut().find(|x| x.pull_request_id == id) {
								*slot = pr.clone();
							}
							p.pr = pr;
							p.file = p.file.min(detail.changes.len().saturating_sub(1));
							p.detail = Some(detail);
							p.error = None;
							// Comments may have changed; file sides have not (same iteration or not, they reload on demand).
							p.files.clear();
						},
						Err(e) => p.error = Some(e),
					}
					if self.page.as_ref().is_some_and(|p| p.tab == PrTab::Files) {
						self.load_file();
					}
					self.prefetch_threads();
				},
				Reply::File(id, path, r) => {
					let Some(p) = self.page.as_mut().filter(|p| p.pr.pull_request_id == id) else { continue };
					let context = if p.full { usize::MAX / 4 } else { CONTEXT };
					let state = match r {
						Ok((Some(a), Some(b))) => {
							let diff = diff::build(&a, &b, context);
							FileState::Ready { sides: (Some(a), Some(b)), diff }
						},
						Ok(sides) => FileState::Ready { sides, diff: FileDiff::default() },
						Err(e) => FileState::Failed(e),
					};
					let current = p.change().is_some_and(|c| c.path() == path);
					p.files.insert(path, state);
					// The whole file opens at its first change.
					let go = current && p.pending.is_none() && (p.go.is_some() || p.full);
					if go {
						p.go.get_or_insert(true);
					}
					if current && let Some(id) = p.pending.take() {
						self.scroll_req = Some(ScrollReq::Reveal(format!("{FILES_DS}.t{id}")));
					} else if go {
						self.finish_go();
					}
				},
				Reply::Wrote(id, what, r) => match r {
					Ok(()) => {
						self.note = format!("{what} sent");
						if let Some(p) = self.page.as_ref().filter(|p| p.pr.pull_request_id == id) {
							let pr = p.pr.clone();
							self.start(Job::Detail(pr));
						}
					},
					Err(e) => self.note = format!("{what} failed: {e}"),
				},
			}
		}
		changed
	}

	// ---- layout numbers -----------------------------------------------------------------------

	/// Lines for the main region (the dock takes the rest).
	fn avail(&self) -> f64 {
		let in_dock = self.compose.as_ref().is_some_and(|c| !matches!(c.kind, ComposeKind::ReviewPrompt(_) | ComposeKind::ReviewAgent));
		let mut dock = 2.0 + if in_dock || self.voting { 2.2 } else { 0.0 };
		if let Some(c) = self.compose.as_ref().filter(|c| c.kind == ComposeKind::Jump) {
			dock += 1.45 * self.jump_results(&c.draft).len().clamp(1, JUMP_ROWS) as f64 + 0.8;
		}
		(self.rows - dock - 0.5).max(6.0)
	}

	/// How many 22px rows fit in `lines` lines.
	fn rows_in(&self, lines: f64) -> usize {
		((lines * self.cell_h * CSS_PER_CELL_H - 10.0) / ROW_PX).floor().max(1.0) as usize
	}

	fn list_lines(&self) -> f64 {
		self.avail() - 2.8
	}

	fn page_body_lines(&self) -> f64 {
		self.avail() - 6.0
	}

	/// The overview has room for the reviewers beside it.
	fn wide(&self) -> bool {
		self.cols >= 130.0
	}


	// ---- views ----------------------------------------------------------------------------------

	fn view(&self) -> View<Msg> {
		let main = if self.client.is_none() {
			ui::col().child(ui::md(
				"**Azure DevOps pull requests**\n\nEnter your organization below: its name, a `dev.azure.com` URL, or any repo URL in it. \
				 Sign-in uses the Azure CLI (`az login`), or a PAT saved with `tern-azdo pat`.",
			))
		} else {
			self.shell()
		};
		let mut dock: Vec<tern_sdk::Node<Msg>> = Vec::new();
		if self.voting {
			let mut row = html::div().class("prompt").child(html::span("Vote").class("pl"));
			for (k, label) in [("1", "Approve"), ("2", "Approve with suggestions"), ("3", "Wait for author"), ("4", "Reject"), ("0", "Reset")] {
				row = row.child(html::span(format!("{k} {label}")));
			}
			dock.push(row.child(html::span("Esc cancels").class("dim")).into());
		}
		if let Some(c) = self.compose.as_ref().filter(|c| c.kind == ComposeKind::Jump) {
			let ids = self.jump_results(&c.draft);
			let at = self.jump.min(ids.len().saturating_sub(1));
			let first = at.saturating_sub(JUMP_ROWS - 1).min(ids.len().saturating_sub(JUMP_ROWS));
			let mut list = html::div().class("jump");
			for id in ids.iter().skip(first).take(JUMP_ROWS) {
				let Some(p) = self.prs.iter().find(|p| p.pull_request_id == *id) else { continue };
				let cur = ids.get(at) == Some(id);
				list = list.child(
					html::div()
						.class(if cur { "row cur" } else { "row" })
						.on_click(Msg::Open(*id))
						.child(html::span(format!("!{id}")).class("id"))
						.child(html::span(p.title.clone()).class("t"))
						.child(html::span(format!("{} \u{b7} {}", p.repository.project.name, short_name(&p.created_by.display_name))).class("pj")),
				);
			}
			if ids.is_empty() {
				list = list.child(html::div().class("row").child(html::span("No pull request matches.").class("dim")));
			}
			dock.push(list.into());
		}
		if let Some(c) = self.compose.as_ref().filter(|c| !matches!(c.kind, ComposeKind::Inline { .. } | ComposeKind::ReviewPrompt(_) | ComposeKind::ReviewAgent)) {
			let file = self.page.as_ref().and_then(Page::change).map(|c| c.path().rsplit('/').next().unwrap_or_default().to_owned()).unwrap_or_default();
			let placeholder: String = match c.kind {
				ComposeKind::Org => "Organization, e.g. Contoso or https://dev.azure.com/Contoso (Enter saves)".into(),
				ComposeKind::ReviewAgent => "Agent: claude, codex, or a command line with {prompt} where the prompt goes (Enter saves, Esc cancels)".into(),
				ComposeKind::ReviewPrompt(k) => format!("The {} prompt for this repo (Enter saves, Shift+Enter new line, Ctrl+P preview, Esc cancels)", k.label()),
				ComposeKind::Jump => "Jump to a PR: type part of its title, author, repo or id (Up/Down choose, Enter opens, Esc cancels)".into(),
				ComposeKind::Filter => "Filter by title, author, repo, branch or id (Enter keeps, Esc clears)".into(),
				ComposeKind::Comment(None) => "New comment on the pull request (Enter sends, Shift+Enter new line, Esc cancels)".into(),
				ComposeKind::Comment(Some(_)) => "Reply (Enter sends, Shift+Enter new line, Esc cancels)".into(),
				ComposeKind::Line => format!("Line of {file} to comment on, -12 for a removed line, or click a line number in the diff (Enter goes on, Esc cancels)"),
				ComposeKind::Inline { line, left } => {
					format!("Comment on {}line {line} of {file} (Enter sends, Shift+Enter new line, Esc cancels)", if left { "removed " } else { "" })
				},
			};
			dock.push(
				ui::editor()
					.key("ed")
					.text(c.draft.clone())
					.cursor(c.utf16_cursor())
					.placeholder(placeholder)
					.sendable(true)
					.max_lines(6u32)
					.on_send(|t| Msg::Send(t.to_owned()))
					.on_edit(|e| Msg::Edit(e.clone()))
					.on_focus(Msg::Focus)
					.into(),
			);
		}
		dock.push(self.status().into());
		View::new().main(main).dock(dock)
	}

	fn status(&self) -> ui::StatusBar<Msg> {
		let org = self.client.as_ref().map_or("not set", |c| config::short(&c.org)).to_owned();
		let mut s = ui::status().key("bar").child(ui::seg(org.clone()).icon("git-pull-request"));
		if let Some(me) = &self.me {
			s = s.child(ui::seg(me.display_name.clone()).tone(Tone::Muted));
		}
		if self.busy > 0 {
			s = s.child(ui::seg("loading…").tone(Tone::Muted));
		}
		// A message takes the key hint's place: the bar drops what does not fit.
		if let Some(e) = &self.error {
			return s.child(ui::seg(first_line(e, 140)).tone(Tone::Error));
		}
		if !self.note.is_empty() {
			return s.child(ui::seg(self.note.clone()));
		}
		let files = self.page.as_ref().is_some_and(|p| p.tab == PrTab::Files);
		// Longest hint that fits beside the org and user segments; the shortest tiers keep the essentials.
		let tiers: [&str; 4] = if files {
			[
				"Esc back · / jump to PR · [ ] prev/next PR · Tab overview · j/k file · h/l fold · ^d/^u gg/G scroll · { } change · s split · x whole file · n next comment · c comment on a line · a reply · i AI review",
				"Esc back · / jump · [ ] PR · Tab overview · j/k file · { } change · s split · x whole · n next · c comment · a reply · i AI",
				"Esc back · / jump · Tab · j/k · s split · n next · c · a",
				"Esc back · Tab · j/k",
			]
		} else if self.page.is_some() {
			[
				"Esc back · / jump to PR · [ ] prev/next PR · Tab files · j/k ^d/^u gg/G scroll · n next comment · a reply · c comment · v vote · i AI review · t context · e explain · w reply · o browser · r refresh",
				"Esc back · / jump · [ ] PR · Tab files · j/k scroll · n next · a reply · c comment · v vote · i AI · o browser",
				"Esc back · / jump · Tab · j/k · n next · a · c · v · o",
				"Esc back · Tab · j/k",
			]
		} else {
			[
				"Enter open · / jump · j/k gg/G · 1-3 tabs · f filter · o browser · y copy link · r refresh · q quit",
				"Enter open · / jump · j/k · 1-3 tabs · f filter · o browser · y link · r refresh",
				"Enter open · / jump · j/k · 1-3 · f filter · o · r",
				"Enter · j/k · f · q",
			]
		};
		let left = org.chars().count() + self.me.as_ref().map_or(0, |m| m.display_name.chars().count()) + 12;
		let room = (self.cols as usize).saturating_sub(left);
		let keys = tiers.iter().find(|t| t.chars().count() <= room).unwrap_or(&tiers[3]);
		s.child(ui::seg(*keys).side(Side::Right).tone(Tone::Muted))
	}

	/// The global filters as three dropdowns: which PRs (Reviewing, Created by me, All active), which project, which status.
	fn topbar(&self) -> html::El<Msg> {
		// A dropdown: its button (what is picked now), and when open its menu of (label, count, picked, click) items.
		let dd = |kind: Menu, title: &str, value: String, items: Vec<(String, Option<usize>, bool, Msg)>| {
			let open = self.menu == Some(kind);
			let mut d = html::div().class("dd").child(
				html::div()
					.class(if open { "ddb on" } else { "ddb" })
					.on_click(Msg::Menu(kind))
					.child(html::span(title.to_owned()).class("tbl"))
					.child(html::span(value))
					.child(html::span(if open { "\u{25b4}" } else { "\u{25be}" }).class("az")),
			);
			if open {
				let mut m = html::div().class("ddm");
				for (label, n, on, msg) in items {
					let mut row = html::div().class(if on { "ddi on" } else { "ddi" }).on_click(msg).child(html::span(label));
					if let Some(n) = n {
						row = row.child(html::span(n.to_string()).class("n"));
					}
					m = m.child(row);
				}
				d = d.child(m);
			}
			d
		};
		let show_items: Vec<_> = ListTab::ALL
			.iter()
			.map(|t| (t.label().to_owned(), Some(self.prs.iter().filter(|p| self.in_tab(p, *t)).count()), *t == self.tab, Msg::ListTab(t.id().to_owned())))
			.collect();
		let tab_n = self.prs.iter().filter(|p| self.in_tab(p, self.tab)).count();
		let pool: Vec<&Pr> = self.prs.iter().filter(|p| self.in_tab(p, self.tab) && self.status.matches(p)).collect();
		let mut names: Vec<&str> = pool.iter().map(|p| p.repository.project.name.as_str()).collect();
		names.sort_by_key(|n| n.to_lowercase());
		names.dedup();
		let mut project_items = vec![("All projects".to_owned(), Some(pool.len()), self.project.is_none(), Msg::Project(None))];
		for n in names {
			let count = pool.iter().filter(|p| p.repository.project.name == n).count();
			project_items.push((n.to_owned(), Some(count), self.project.as_deref() == Some(n), Msg::Project(Some(n.to_owned()))));
		}
		let status_items: Vec<_> = StatusFilter::ALL.iter().map(|f| (f.label().to_owned(), None, *f == self.status, Msg::Status(f.id().to_owned()))).collect();
		let sort_items: Vec<_> = SortMode::ALL.iter().map(|m| (m.label().to_owned(), None, *m == self.sort, Msg::Sort(m.id().to_owned()))).collect();
		html::div()
			.class("topbar")
			.child(dd(Menu::Show, "Show", format!("{} ({tab_n})", self.tab.label()), show_items))
			.child(dd(Menu::Project, "Project", self.project.clone().unwrap_or_else(|| "All projects".to_owned()), project_items))
			.child(dd(Menu::Status, "Status", self.status.label().to_owned(), status_items))
			.child(dd(Menu::Sort, "Sort", self.sort.label().to_owned(), sort_items))
	}

	/// The navigation rail: the PRs the filters leave, grouped by project, each as a card with the whole title, author,
	/// age and votes.
	fn rail(&self) -> html::El<Msg> {
		let mut rail = html::div().class("rail");
		let cur = self.page.as_ref().map(|p| p.pr.pull_request_id).or(self.sel);
		let visible = self.visible();
		if visible.is_empty() {
			let msg = if self.loading && self.prs.is_empty() { "Loading pull requests\u{2026}" } else { "No pull requests match these filters." };
			return rail.child(html::div().class("row rl").child(html::span(msg)));
		}
		let me = self.me_id();
		let mut last = "";
		for q in &visible {
			let name = q.repository.project.name.as_str();
			if self.sort == SortMode::Project && name != last {
				last = name;
				let n = visible.iter().filter(|p| p.repository.project.name == name).count();
				rail = rail.child(html::div().class("row ph").child(html::span(name.to_owned()).class("nm")).child(html::span(n.to_string()).class("n")));
			}
			let id = q.pull_request_id;
			let mut meta = html::div().class("rm").child(html::span(format!("!{id}")));
			if q.is_draft {
				meta = meta.child(html::span("draft").class("chip"));
			}
			if self.sort != SortMode::Project {
				meta = meta.child(html::span(name.to_owned()));
			}
			meta = meta.child(html::span(short_name(&q.created_by.display_name))).child(html::span(age(&q.creation_date)));
			let counts = api::vote_counts(q);
			for (vote, class, glyph) in [(10, "ok", "\u{2713}"), (5, "ok", "\u{2713}~"), (-5, "wait", "\u{23f8}"), (-10, "bad", "\u{2717}")] {
				if let Some(n) = counts.get(&vote) {
					meta = meta.child(html::span(format!("{glyph}{n}")).class(class));
				}
			}
			if let Some(v) = q.vote_of(me).filter(|v| *v != 0 && q.created_by.id != me) {
				let (g, class) = vote_glyph(v);
				meta = meta.child(html::span(format!("you {g}")).class(class));
			}
			rail = rail.child(
				html::div()
					.class(if Some(id) == cur { "rpr cur" } else { "rpr" })
					.on_click(Msg::Open(id))
					.child(html::div().class("rt").child(html::span(q.title.clone())))
					.child(meta),
			);
		}
		rail
	}

	/// The whole screen: the filter bar over the navigation rail and the open pull request (the rail alone on a narrow
	/// pane until one is opened).
	fn shell(&self) -> ui::Col<Msg> {
		let wide = self.cols >= RAIL_MIN_COLS;
		// It grows to fill the native column; without that it is only as tall as its content (the rail).
		let mut shell = html::div().class("shell").grow(1.0);
		match (&self.page, wide) {
			(Some(p), true) => shell = shell.child(self.topbar()).child(html::div().class("lw").child(self.rail()).child(self.page_pane(p))),
			(Some(p), false) => shell = shell.child(self.page_pane(p)),
			(None, true) => {
				let msg = if self.loading && self.prs.is_empty() { "Loading pull requests\u{2026}" } else { "Pick a pull request." };
				let pane = html::div().class("lwm").child(html::div().class("azmsg").child(html::span(msg)));
				shell = shell.child(self.topbar()).child(html::div().class("lw").child(self.rail()).child(pane));
			},
			(None, false) => shell = shell.child(self.topbar()).child(self.rail().class("rail full")),
		}
		ui::col().key("pbody").child(shell)
	}

	/// One pull request: its title, author and branches, the tabs, then the page. Nothing here moves when you switch PRs.
	fn page_pane(&self, p: &Page) -> html::El<Msg> {
		let pr = &p.pr;
		let mut head = html::div().class("head").child(html::span(format!("!{}", pr.pull_request_id)).class("id")).child(html::span(pr.title.clone()).class("title"));
		if pr.is_draft {
			head = head.child(html::span("draft").class("chip"));
		}
		if let Some(m) = pr.merge_status.as_deref().filter(|m| *m == "conflicts" || *m == "failure") {
			head = head.child(html::span(if m == "conflicts" { "conflicts" } else { "merge failed" }).class("chip bad"));
		}
		let sub = html::div()
			.class("head hsub")
			.child(avatar(&pr.created_by.display_name))
			.child(person(&pr.created_by.display_name))
			.child(html::span("wants to merge").class("dim"))
			.child(html::span(pr.source().to_owned()).class("br"))
			.child(html::span("into").class("dim"))
			.child(html::span(pr.target().to_owned()).class("br"))
			.child(html::span(format!("{} / {}", pr.repository.project.name, pr.repository.name)).class("dim right"))
			.child(html::span("\u{b7}").class("dim"))
			.child(time(&pr.creation_date, ""));
		let files = p.detail.as_ref().map_or(0, |d| d.changes.len());
		let tab = |id: &str, label: String, on: bool| html::span(label).class(if on { "ptab on" } else { "ptab" }).on_click(Msg::PrTab(id.to_owned()));
		let tabs = html::div()
			.class("ptabs")
			.child(tab("overview", "Overview".into(), p.tab != PrTab::Files))
			.child(tab("files", format!("Files ({files})"), p.tab == PrTab::Files))
			.child(html::span("Review with AI").class("rvb").title("Start an AI agent that reviews this pull request (i)").on_click(Msg::Ai(review::Kind::Review, None)))
			.child(html::span("Add as context").class("rvb sec").title("Start an AI agent that has read this pull request and waits for your questions (t)").on_click(Msg::Ai(review::Kind::Context, None)))
			.child(self.review_menu());
		let body: tern_sdk::Node<Msg> = if let Some(e) = &p.error {
			html::div().class("box scroll").child(ui::md(format!("Could not load this pull request.\n\n{e}"))).into()
		} else if p.detail.is_none() {
			html::div().class("box scroll").child(html::span("Loading\u{2026}").class("dim")).into()
		} else if p.tab == PrTab::Files {
			self.files_view(p).into()
		} else {
			self.overview(p).into()
		};
		html::div().class("lwm").child(head).child(sub).child(tabs).child(html::div().class("pbb").child(body))
	}

	/// The gear beside the AI buttons and the menu it opens: edit each prompt, or the checkout and agent.
	fn review_menu(&self) -> html::El<Msg> {
		let mut d = html::div().class("dd rvd").child(
			html::span("\u{2699}")
				.class(if self.rv_menu { "rvg on" } else { "rvg" })
				.title("AI settings for this repo: prompts, checkout, agent (I)")
				.on_click(Msg::ReviewMenu),
		);
		if self.rv_menu {
			let mut m = html::div().class("ddm");
			for k in review::Kind::ALL {
				m = m.child(html::div().class("ddi").on_click(Msg::EditPrompt(k)).child(html::span(format!("Edit the {} prompt", k.label()))));
			}
			m = m.child(html::div().class("ddi").on_click(Msg::ReviewSetup).child(html::span("Checkout folder")));
			m = m.child(html::div().class("ddi").on_click(Msg::ReviewAgent).child(html::span("Agent")));
			d = d.child(m);
		}
		if let Some(panel) = self.ai_panel(AiAnchor::Header) {
			d = d.child(panel);
		}
		d
	}

	/// A comment thread as a card: where it is, the code it is about, then every comment in full.
	fn thread_card(&self, p: &Page, t: &Thread, with_code: bool) -> html::El<Msg> {
		let selected = p.thread == Some(t.id);
		let (label, tone) = api::status_label(t.status.as_deref());
		let mut head = html::div().class("thd");
		if let Some(path) = t.file() {
			let (dir, file) = path.rsplit_once('/').unwrap_or(("", path));
			head = head.child(html::span(file.to_owned()).class("fname")).child(html::span(dir.trim_start_matches('/').to_owned()).class("dim fdir"));
			if let (Some(a), Some(b)) = (t.line(), t.line_end()) {
				head = head.child(html::span(if b > a { format!("lines {a}-{b}") } else { format!("line {a}") }).class("ln"));
			}
		} else {
			head = head.child(html::span("Comment on the pull request").class("dim"));
		}
		head = head.child(html::span(label).class(format!("chip {tone} right")));
		let mut card = html::div().class(if selected { "card cur" } else { "card" }).on_click(Msg::Thread(t.id)).child(head);
		if with_code && let Some(hunks) = p.snippet(t) {
			let diff = ui::diff().key(format!("sn{}", t.id)).hunks(hunks).path(t.file().unwrap_or_default().to_owned()).mode(ui::DiffMode::Unified);
			card = card.child(html::div().class("snip").child(diff));
		}
		for (i, c) in t.human().enumerate() {
			let when = c.published_date.as_deref().unwrap_or("");
			let mut meta = html::div().class("cmh").child(person(&c.author.display_name)).child(time(when, ""));
			if c.last_content_updated_date.as_deref().is_some_and(|u| stamp(u) - stamp(when) > 60_000) {
				meta = meta.child(html::span("edited").class("when"));
			}
			let body = html::div().class("cmb").child(meta).child(ui::md(c.content.clone().unwrap_or_default()));
			card = card.child(html::div().class(if i == 0 { "cm" } else { "cm rep" }).child(avatar(&c.author.display_name)).child(body));
		}
		card = card.child(
			html::div()
				.class("cact")
				.child(html::span("Explain").class("cab").title("Ask an AI agent to explain this comment (e)").on_click(Msg::Ai(review::Kind::Explain, Some(t.id))))
				.child(html::span("Respond with AI").class("cab").title("Ask an AI agent to draft a reply (w)").on_click(Msg::Ai(review::Kind::Respond, Some(t.id)))),
		);
		if let Some(panel) = self.ai_panel(AiAnchor::Thread(t.id)) {
			card = card.child(panel);
		}
		if selected {
			card = card.child(html::div().class("hint").child(html::span("a reply · e explain · w AI reply · n next · Esc done")));
		}
		card
	}

	/// Where the PR stands: votes, conflicts, open comments.
	fn status_card(&self, p: &Page) -> html::El<Msg> {
		let pr = &p.pr;
		let names = |pick: &dyn Fn(&api::Reviewer) -> bool| pr.reviewers.iter().filter(|r| pick(r)).map(|r| r.display_name.as_str()).collect::<Vec<_>>();
		let mut rows: Vec<(&str, &str, String)> = Vec::new();
		if pr.is_draft {
			rows.push(("✎", "dim", "Draft: not ready for review yet".into()));
		}
		let rejected = names(&|r| r.vote == -10);
		if !rejected.is_empty() {
			rows.push(("✗", "bad", format!("Rejected by {}", rejected.join(", "))));
		}
		let waiting = names(&|r| r.vote == -5);
		if !waiting.is_empty() {
			rows.push(("⏸", "wait", format!("{} {} waiting for the author", waiting.join(", "), if waiting.len() == 1 { "is" } else { "are" })));
		}
		let required = names(&|r| r.is_required && r.vote < 5);
		if !required.is_empty() {
			rows.push(("○", "wait", format!("Waiting on {} (required)", required.join(", "))));
		}
		rows.push(match pr.reviewers.iter().filter(|r| r.vote >= 5).count() {
			0 => ("○", "dim", "No approvals yet".into()),
			1 => ("✓", "ok", "1 reviewer approved".into()),
			n => ("✓", "ok", format!("{n} reviewers approved")),
		});
		rows.push(match pr.merge_status.as_deref() {
			Some("conflicts") => ("✗", "bad", "Merge conflicts with the target branch".into()),
			Some("failure") => ("✗", "bad", "The merge check failed".into()),
			Some("succeeded") => ("✓", "ok", "No merge conflicts".into()),
			_ => ("○", "dim", "Checking for merge conflicts".into()),
		});
		let threads = p.all_threads();
		let open = threads.iter().filter(|t| t.is_open()).count();
		if !threads.is_empty() {
			rows.push(if open == 0 {
				("✓", "ok", "All comments resolved".into())
			} else {
				("●", "acc", format!("{open} of {} comment threads active", threads.len()))
			});
		}
		html::div().class("card").child(html::div().class("ch").child(html::span("Review status"))).children(
			rows.into_iter().map(|(g, class, text)| html::div().class("st").child(html::span(g).class(format!("gl {class}"))).child(html::span(text))),
		)
	}

	fn reviewers_card(&self, pr: &Pr) -> html::El<Msg> {
		let mut card = html::div().class("card").child(html::div().class("ch").child(html::span("Reviewers")));
		if pr.reviewers.is_empty() {
			return card.child(html::span("No reviewers").class("dim"));
		}
		let mut rs: Vec<&api::Reviewer> = pr.reviewers.iter().collect();
		rs.sort_by_key(|r| r.display_name.to_lowercase());
		for (label, required) in [("Required", true), ("Optional", false)] {
			let group: Vec<_> = rs.iter().filter(|r| r.is_required == required).collect();
			if group.is_empty() {
				continue;
			}
			card = card.child(html::div().class("grh").child(html::span(label)));
			for r in group {
				let (g, class) = vote_glyph(r.vote);
				let who = html::div().child(html::div().child(html::span(r.display_name.clone()))).child(html::div().class("dim small").child(html::span(api::vote_label(r.vote))));
				card = card.child(html::div().class("rv").child(html::span(g).class(format!("vg {class}"))).child(avatar(&r.display_name)).child(who));
			}
		}
		card
	}

	fn work_items_card(&self, p: &Page) -> Option<html::El<Msg>> {
		let d = p.detail.as_ref().filter(|d| !d.work_items.is_empty())?;
		let mut card = html::div().class("card").child(html::div().class("ch").child(html::span("Work items")));
		for w in &d.work_items {
			let tone = match w.state.as_str() {
				"Closed" | "Done" | "Resolved" | "Removed" => "ok",
				"New" | "To Do" => "dim",
				_ => "acc",
			};
			card = card.child(
				html::div()
					.class("wi")
					.child(html::div().child(html::span(format!("{} {}", w.kind, w.id)).class("wk")))
					.child(html::div().child(html::span(w.title.clone())))
					.child(html::div().class("small").child(html::span("● ").class(tone)).child(html::span(w.state.clone()).class("dim"))),
			);
		}
		Some(card)
	}

	fn overview(&self, p: &Page) -> html::El<Msg> {
		let pr = &p.pr;
		let wide = self.wide();
		let mut blocks: Vec<tern_sdk::Node<Msg>> = vec![self.status_card(p).key("status").into()];
		if !wide {
			blocks.push(self.reviewers_card(pr).into());
			if let Some(w) = self.work_items_card(p) {
				blocks.push(w.into());
			}
		}
		let desc = pr.description.clone().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| "_No description._".into());
		blocks.push(html::div().class("card").child(html::div().class("ch").child(html::span("Description"))).child(ui::md(desc)).into());
		blocks.push(
			html::div()
				.class("ah")
				.child(html::span("Activity").class("at"))
				.child(html::span("newest first · n jumps between comments · c adds one").class("dim small"))
				.into(),
		);
		let items = p.timeline();
		let n = items.len();
		for (i, item) in items.iter().enumerate() {
			let last = i + 1 == n;
			let node = match item {
				Item::Thread(t) => tl_item(if t.is_open() { "acc" } else { "ok" }, last, self.thread_card(p, t, true)).key(format!("t{}", t.id)),
				Item::Event(e, when) => {
					let dot = match e.vote {
						Some(10 | 5) => "ok",
						Some(-5) => "wait",
						Some(-10) => "bad",
						_ => "",
					};
					tl_item(dot, last, event_row(&e.who, &e.what, when))
				},
				Item::Created => tl_item("acc", last, event_row(&pr.created_by.display_name, "created the pull request", &pr.creation_date)),
			};
			blocks.push(node.into());
		}
		let mut ov = html::div().key("ov").class("ov").child(html::div().key("ovm").class("ovmain").children(blocks));
		if wide {
			ov = ov.child(html::div().class("ovside").child(self.reviewers_card(pr)).children(self.work_items_card(p)));
		}
		ov
	}

	fn files_view(&self, p: &Page) -> html::El<Msg> {
		let Some(d) = &p.detail else { return html::div().key("fv") };
		if d.changes.is_empty() {
			return html::div().key("fv").class("azmsg").child(html::span("No files changed."));
		}
		let counts: HashMap<&str, usize> = d.threads.iter().filter(|t| t.shown()).filter_map(Thread::file).fold(HashMap::new(), |mut m, f| {
			*m.entry(f).or_default() += 1;
			m
		});
		let paths = p.paths();
		let rows = tree::rows(&paths, &p.folded).into_iter().map(|r| -> tern_sdk::Node<Msg> {
			match r {
				tree::Row::Dir { path, label, depth, open } => html::div()
					.key(dir_key(&path))
					.class(format!("aztr azdir azd{}", depth.min(9)))
					.on_click(Msg::Folder(path))
					.child(html::span(if open { "▾" } else { "▸" }).class("azch"))
					.child(html::span(label))
					.into(),
				tree::Row::File { index, name, depth } => {
					let c = &d.changes[index];
					let mut row = html::div()
						.key(format!("f{index}"))
						.class(format!("aztr azd{}{}", depth.min(9), if index == p.file { " cur" } else { "" }))
						.on_click(Msg::File(index))
						.child(html::span(c.letter()).class(format!("fk {}", c.letter())))
						.child(html::span(name).class("fn"));
					if let Some(n) = counts.get(c.path()) {
						row = row.child(html::span(format!("💬{n}")).class("cc right"));
					}
					row.into()
				},
			}
		});
		html::div().key("fv").class("azfv").child(html::div().key("tree").class("aztree").children(rows)).child(self.diff_pane(p))
	}

	/// The selected file: its header, then its diff with each comment thread's card under the lines it is about.
	fn diff_pane(&self, p: &Page) -> html::El<Msg> {
		let Some(c) = p.change() else { return html::div().key("dp").class("azdp") };
		let path = c.path();
		let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
		let mut head = html::div()
			.class("azdh")
			.child(html::span(c.letter()).class(format!("fk {}", c.letter())))
			.child(html::span(name.to_owned()).class("fname"))
			.child(html::span(dir.trim_start_matches('/').to_owned()).class("dim fdir"));
		let threads = p.file_threads();
		let card = |t: &Thread| -> tern_sdk::Node<Msg> { html::div().key(format!("t{}", t.id)).class("azic").child(self.thread_card(p, t, false)).into() };
		let note = |text: &str| -> tern_sdk::Node<Msg> { html::div().class("azmsg").child(html::span(text.to_owned())).into() };
		let mut body: Vec<tern_sdk::Node<Msg>> = vec![html::div().key("top").into()];
		match p.files.get(path) {
			None | Some(FileState::Loading) => body.push(note("Loading…")),
			Some(FileState::Failed(e)) => body.push(note(e)),
			Some(FileState::Ready { sides, diff }) => {
				if sides.0.is_none() || sides.1.is_none() || diff.is_empty() {
					body.push(note(if sides.0.is_none() || sides.1.is_none() {
						"Binary or very large file: open it in the browser (o)."
					} else if c.is("rename") {
						"Renamed, no content changes."
					} else {
						"No content changes."
					}));
					body.extend(threads.iter().map(|t| card(t)));
				} else {
					head = head.child(html::span(format!("+{} −{}", diff.added(), diff.removed())).class("dim"));
					let lang = name.rsplit_once('.').map_or("", |(_, ext)| ext);
					// Thread cards, and the open comment box (None), each after the lines it is about.
					let mut placed: Vec<(usize, Option<&Thread>)> = threads
						.iter()
						.map(|t| {
							let at = t.line().map_or(0, |l| {
								let l = u64::from(l);
								diff.anchor(l, t.line_end().map_or(l, u64::from).max(l), t.on_left())
							});
							(at, Some(*t))
						})
						.collect();
					let open = self.compose.as_ref().and_then(|c| match c.kind {
						ComposeKind::Inline { line, left } => Some((c, line, left)),
						_ => None,
					});
					if let Some((_, line, left)) = open {
						placed.push((diff.anchor(line.into(), line.into(), left), None));
					}
					placed.sort_by_key(|(at, t)| (*at, t.is_some(), t.and_then(|t| t.line())));
					// Where a jump to each change scrolls to: the view is cut there, so those places have keys.
					let cuts: Vec<usize> = diff.changes().into_iter().map(|c| diff.jump_to(c, JUMP_LEAD)).collect();
					let segs = |body: &mut Vec<tern_sdk::Node<Msg>>, from: usize, to: usize| {
						let mut a = from;
						for &c in cuts.iter().filter(|&&c| c > from && c < to) {
							if c > a {
								body.push(segment(diff, a, c, p.split, lang));
								a = c;
							}
						}
						body.push(segment(diff, a, to, p.split, lang));
					};
					let mut from = 0;
					for (at, t) in placed {
						if at > from {
							segs(&mut body, from, at);
							from = at;
						}
						body.push(match (t, open) {
							(Some(t), _) => card(t),
							(None, Some((c, line, left))) => self.line_box(c, line, left),
							(None, None) => continue,
						});
					}
					if from < diff.len() {
						segs(&mut body, from, diff.len());
					}
				}
			},
		}
		let tog = |on: bool, label: &str, m: Msg| html::span(label.to_owned()).class(if on { "on" } else { "" }).on_click(m);
		head = head
			.child(html::div().class("aztog right").child(tog(!p.split, "Inline", Msg::Split(false))).child(tog(p.split, "Side by side", Msg::Split(true))))
			.child(html::div().class("aztog").child(tog(!p.full, "Changes", Msg::Full(false))).child(tog(p.full, "Whole file", Msg::Full(true))));
		head = head.child(html::div().class("aztog").child(tog(false, "\u{25b2}", Msg::Change(false))).child(tog(false, "\u{25bc} change", Msg::Change(true))));
		html::div().key("dp").class("azdp").child(head).child(html::div().key("ds").class("azds").children(body))
	}

	/// The comment box under a diff line, as in Azure DevOps.
	fn line_box(&self, c: &Composer, line: u32, left: bool) -> tern_sdk::Node<Msg> {
		let me = self.me.as_ref().map_or("You", |m| m.display_name.as_str());
		html::div()
			.key("cbox")
			.class("azcbox")
			.child(
				html::div()
					.class("azcbh")
					.child(avatar(me))
					.child(html::span(format!("Comment on line {line}{}", if left { " of the old version" } else { "" })).class("dim")),
			)
			.child(
				ui::editor()
					.key("ed")
					.text(c.draft.clone())
					.cursor(c.utf16_cursor())
					.placeholder("Add a comment. Markdown works. Enter sends, Shift+Enter new line, Esc cancels.")
					.sendable(true)
					.max_lines(10u32)
					.on_send(|t| Msg::Send(t.to_owned()))
					.on_edit(|e| Msg::Edit(e.clone()))
					.on_focus(Msg::Focus),
			)
			.child(
				html::div()
					.class("azcbf")
					.child(html::button("Explain with AI").class("azbtn ai").title("Ask an AI agent to explain this code").on_click(Msg::AiLine(review::Kind::ExplainCode)))
					.child(html::button("Review with AI").class("azbtn ai").title("Ask an AI agent to review this code").on_click(Msg::AiLine(review::Kind::ReviewCode)))
					.child(html::button("Cancel").class("azbtn").on_click(Msg::Cancel))
					.child(html::button("Comment").class("azbtn pri").on_click(Msg::Submit)),
			)
			.children(self.ai_panel(AiAnchor::Line))
			.into()
	}

	// ---- input ----------------------------------------------------------------------------------

	/// Moves to the next (or previous) thread here; in Files, scrolls the diff to it.
	fn step_thread(&mut self, forward: bool) {
		let cycle = |at: Option<usize>, n: usize| match (at, forward) {
			(None, true) => 0,
			(None, false) => n - 1,
			(Some(a), true) => (a + 1) % n,
			(Some(a), false) => (a + n - 1) % n,
		};
		let Some(p) = &mut self.page else { return };
		if p.tab == PrTab::Overview {
			let ids: Vec<u32> = p.all_threads().iter().map(|t| t.id).collect();
			if ids.is_empty() {
				self.note = "no comments here".into();
				return;
			}
			let next = ids[cycle(p.thread.and_then(|t| ids.iter().position(|i| *i == t)), ids.len())];
			p.thread = Some(next);
			self.scroll_req = Some(ScrollReq::Reveal(format!("{OV_MAIN}.t{next}")));
			return;
		}
		// Files: every file's threads in tree order, then by line; stepping past a file's last opens the next.
		let Some(d) = &p.detail else { return };
		let paths = p.paths();
		let order = tree::order(&paths);
		let mut all: Vec<(usize, u32)> = Vec::new();
		for &i in &order {
			let mut ts: Vec<&Thread> = d.threads.iter().filter(|t| t.shown() && t.file() == Some(paths[i])).collect();
			ts.sort_by_key(|t| t.line().unwrap_or(0));
			all.extend(ts.into_iter().map(|t| (i, t.id)));
		}
		if all.is_empty() {
			self.note = "no comments on any file".into();
			return;
		}
		let pos = |i: usize| order.iter().position(|x| *x == i).unwrap_or(0);
		let at = p.thread.and_then(|t| all.iter().position(|(_, id)| *id == t));
		let next = match at {
			Some(_) => cycle(at, all.len()),
			// From no thread: the first one in or after this file (or before it, going back).
			None if forward => all.iter().position(|(i, _)| pos(*i) >= pos(p.file)).unwrap_or(0),
			None => all.iter().rposition(|(i, _)| pos(*i) <= pos(p.file)).unwrap_or(all.len() - 1),
		};
		let (file, id) = all[next];
		if file != p.file {
			self.pick_file(file);
		}
		let Some(p) = &mut self.page else { return };
		p.thread = Some(id);
		if p.file_ready() {
			self.scroll_req = Some(ScrollReq::Reveal(format!("{FILES_DS}.t{id}")));
		} else {
			p.pending = Some(id);
		}
	}

	/// Scroll the diff to change `n` of the selected file, with a few lines of the file above it.
	fn reveal_change(&mut self, n: usize) {
		let Some(p) = &mut self.page else { return };
		let Some(diff) = p.diff() else { return };
		let Some(start) = diff.changes().get(n).copied() else { return };
		let at = diff.jump_to(start, JUMP_LEAD);
		p.change = Some(n);
		p.thread = None;
		self.scroll_req = Some(ScrollReq::Reveal(format!("{FILES_DS}.s{at}")));
	}

	/// Carry out a jump that waited for the file: to its first change or its last.
	fn finish_go(&mut self) {
		let Some(p) = &mut self.page else { return };
		let Some(first) = p.go.take() else { return };
		let count = p.diff().map_or(0, |d| d.changes().len());
		if count == 0 {
			self.change_file(first);
		} else {
			self.reveal_change(if first { 0 } else { count - 1 });
		}
	}

	/// Vim's `]c` and `[c`: the next (or previous) change in the diff, going on to the next (or previous) file at the end.
	fn step_change(&mut self, forward: bool) {
		let Some(p) = &self.page else { return };
		if p.tab != PrTab::Files {
			self.note = "changes are in the Files tab".into();
			return;
		}
		if !p.file_ready() {
			self.note = "the file is still loading".into();
			return;
		}
		let count = p.diff().map_or(0, |d| d.changes().len());
		let next = match (p.change, forward) {
			(None, true) => Some(0),
			(None, false) => count.checked_sub(1),
			(Some(c), true) => Some(c + 1),
			(Some(c), false) => c.checked_sub(1),
		}
		.filter(|n| *n < count);
		match next {
			Some(n) => self.reveal_change(n),
			None => self.change_file(forward),
		}
	}

	/// The first change of the next file (the last of the previous one going back), in the file tree's order.
	fn change_file(&mut self, forward: bool) {
		let Some(p) = &self.page else { return };
		let order = tree::order(&p.paths());
		let Some(at) = order.iter().position(|i| *i == p.file) else { return };
		let next = if forward { at + 1 } else { at.wrapping_sub(1) };
		let Some(&file) = order.get(next) else {
			self.note = if forward { "last change" } else { "first change" }.into();
			return;
		};
		self.pick_file(file);
		let Some(p) = &mut self.page else { return };
		p.go = Some(forward);
		if p.file_ready() {
			self.finish_go();
		}
	}

	/// The next (or previous) file in the tree, skipping folded folders.
	fn step_file(&mut self, forward: bool) {
		let Some(p) = &self.page else { return };
		let shown: Vec<usize> = tree::rows(&p.paths(), &p.folded)
			.into_iter()
			.filter_map(|r| match r {
				tree::Row::File { index, .. } => Some(index),
				tree::Row::Dir { .. } => None,
			})
			.collect();
		if shown.is_empty() {
			return;
		}
		let next = match shown.iter().position(|i| *i == p.file) {
			Some(a) if forward => (a + 1).min(shown.len() - 1),
			Some(a) => a.saturating_sub(1),
			None => 0,
		};
		self.pick_file(shown[next]);
	}

	/// Folds (or unfolds) the folder the selected file is in.
	fn fold_here(&mut self, fold: bool) {
		let Some(p) = &mut self.page else { return };
		let parents = tree::parents(&p.paths(), p.file);
		if fold {
			if let Some(inner) = parents.last() {
				p.folded.insert(inner.clone());
			}
		} else {
			for f in &parents {
				p.folded.remove(f);
			}
		}
	}

	fn scroll(&mut self, delta: i64) {
		if self.page.is_none() {
			return;
		}
		// Tern scrolls the overview and the diff itself. Its page step does nothing here, so a page is a screen's worth of line steps.
		let page = (self.page_body_lines() * 0.8) as i64;
		self.scroll_req = Some(match delta {
			d if d <= -10_000 => ScrollReq::By(ScrollBy::Start),
			d if d >= 10_000 => ScrollReq::By(ScrollBy::End),
			d if d < -1 => ScrollReq::Lines(-page),
			d if d > 1 => ScrollReq::Lines(page),
			d => ScrollReq::Lines(d * 3),
		});
	}

	fn pick_file(&mut self, i: usize) {
		let Some(p) = &mut self.page else { return };
		let n = p.detail.as_ref().map_or(0, |d| d.changes.len());
		if n == 0 {
			return;
		}
		p.file = i.min(n - 1);
		p.thread = None;
		p.pending = None;
		p.change = None;
		p.go = None;
		// Unfold the folders it is in, so its row can be shown.
		for f in tree::parents(&p.paths(), p.file) {
			p.folded.remove(&f);
		}
		self.tree_req = Some(format!("{TREE}.f{}", p.file));
		self.scroll_req = Some(ScrollReq::By(ScrollBy::Start));
		self.load_file();
	}

	fn copy(&mut self, text: &str, what: &str) {
		self.note = match crate::clip::set(text) {
			Ok(()) => format!("copied {what}"),
			Err(e) => format!("copy failed: {e}"),
		};
	}

	/// The open PR's repo key and its review settings: what is saved, else the defaults.
	fn review_settings(&self) -> Option<(String, config::Review)> {
		let repo = &self.page.as_ref()?.pr.repository;
		let key = config::repo_key(&repo.project.name, &repo.name);
		let mut r = config::review(&key).unwrap_or_default();
		if r.agent.trim().is_empty() {
			r.agent = review::DEFAULT_AGENT.into();
		}
		Some((key, r))
	}

	/// What a prompt can use: the open PR, and for the comment prompts one thread (the given one, else the selected one,
	/// else the first).
	fn review_vars(&self, thread: Option<u32>, line: Option<&(u32, bool, String)>) -> Option<review::Vars> {
		let p = self.page.as_ref()?;
		let pr = &p.pr;
		let none = |s: String, empty: &str| if s.trim().is_empty() { empty.to_owned() } else { s };
		let at = |t: &Thread| match (t.file(), t.line()) {
			(Some(f), Some(l)) => format!("{}:{l}", f.trim_start_matches('/')),
			(Some(f), None) => f.trim_start_matches('/').to_owned(),
			_ => "the pull request".to_owned(),
		};
		let mut v = review::Vars {
			id: pr.pull_request_id.to_string(),
			title: pr.title.clone(),
			url: self.client.as_ref().map(|c| c.web_url(pr)).unwrap_or_default(),
			project: pr.repository.project.name.clone(),
			repo: pr.repository.name.clone(),
			source: pr.source().to_owned(),
			target: pr.target().to_owned(),
			author: pr.created_by.display_name.clone(),
			description: none(pr.description.clone().unwrap_or_default(), "(none)"),
			..review::Vars::default()
		};
		if let Some(d) = &p.detail {
			let files: Vec<String> = d.changes.iter().take(300).map(|c| format!("{} {}", c.letter(), c.path().trim_start_matches('/'))).collect();
			v.files = files.join("\n");
			let comments: Vec<String> = d
				.threads
				.iter()
				.filter(|t| t.shown())
				.map(|t| {
					let (status, _) = api::status_label(t.status.as_deref());
					let first = t.human().next();
					let who = first.map_or("", |c| c.author.display_name.as_str());
					let text = first_line(first.and_then(|c| c.content.as_deref()).unwrap_or_default(), 300);
					let more = t.human().count() - 1;
					format!("- [{status}] {}: {who}: {text}{}", at(t), if more > 0 { format!(" (+{more} replies)") } else { String::new() })
				})
				.collect();
			v.comments = comments.join("\n");
			let t = if line.is_some() {
				None
			} else {
				thread.or(p.thread).and_then(|id| d.threads.iter().find(|t| t.id == id && t.shown())).or_else(|| d.threads.iter().find(|t| t.shown()))
			};
			if let Some(t) = t {
				let say = |c: &api::Comment| format!("{}: {}", c.author.display_name, c.content.as_deref().unwrap_or_default().trim());
				v.thread = t.human().map(say).collect::<Vec<_>>().join("\n\n");
				v.file = t.file().map_or_else(|| "the pull request".to_owned(), |f| f.trim_start_matches('/').to_owned());
				v.line = t.line().map_or_else(|| "n/a".to_owned(), |l| l.to_string());
				v.comment = t.human().last().and_then(|c| c.content.clone()).unwrap_or_default();
				v.code = Self::code_at(p, t.file().unwrap_or_default(), t.line().unwrap_or(0), t.on_left());
			}
		}
		if let Some((l, left, draft)) = line
			&& let Some(c) = p.change()
		{
			v.file = c.path().trim_start_matches('/').to_owned();
			v.line = l.to_string();
			v.comment = if draft.trim().is_empty() { "(none)".into() } else { draft.clone() };
			v.code = Self::code_at(p, c.path(), *l, *left);
		}
		v.files = none(v.files, "(still loading)");
		v.comments = none(v.comments, "(none)");
		v.thread = none(v.thread, "(no comment)");
		v.file = none(v.file, "n/a");
		v.line = none(v.line, "n/a");
		v.code = none(v.code, "(not loaded)");
		Some(v)
	}

	/// The lines around `line` of a loaded file, numbered, with `line` marked.
	fn code_at(p: &Page, path: &str, line: u32, left: bool) -> String {
		let Some(FileState::Ready { sides, .. }) = p.files.get(path) else { return String::new() };
		let Some(text) = (if left { &sides.0 } else { &sides.1 }).as_ref() else { return String::new() };
		let lines: Vec<&str> = text.lines().collect();
		let n = line as usize;
		if n == 0 || n > lines.len() {
			return String::new();
		}
		(n.saturating_sub(5).max(1)..=(n + 5).min(lines.len()))
			.map(|i| format!("{}{i:>5}: {}", if i == n { ">" } else { " " }, lines[i - 1]))
			.collect::<Vec<_>>()
			.join("\n")
	}

	/// Show the checkout picker, starting at the saved folder, else the clone found from where the plugin started, else
	/// that folder; then run `next`, if any, once a folder is chosen.
	fn review_setup(&mut self, next: Option<AiReq>) {
		let Some((key, r)) = self.review_settings() else { return };
		let repo = self.page.as_ref().map(|p| p.pr.repository.name.clone()).unwrap_or_default();
		let start = Some(r.dir.trim().to_owned())
			.filter(|d| std::path::Path::new(d).is_dir())
			.or_else(|| review::find_checkout(&repo))
			.or_else(|| std::env::current_dir().ok().map(|d| review::show(&d)))
			.unwrap_or_default();
		self.note = format!("pick the folder of the {repo} clone");
		self.rv_menu = false;
		self.pick = Some(std::path::PathBuf::from(start));
		self.pick_sel = 0;
		self.pick_q.clear();
		self.ai_at = next.as_ref().map_or(AiAnchor::Header, AiReq::anchor);
		// Bring the comment (and the panel in it) into view.
		if let AiAnchor::Thread(id) = self.ai_at {
			let files = self.page.as_ref().is_some_and(|p| p.tab == PrTab::Files);
			self.scroll_req = Some(ScrollReq::Reveal(format!("{}.t{id}", if files { FILES_DS } else { OV_MAIN })));
		}
		self.review = r;
		self.review_key = key;
		self.review_next = next;
	}

	/// The checkout picker or the prompt or agent editor, as a panel next to what asked for it.
	fn ai_panel(&self, at: AiAnchor) -> Option<html::El<Msg>> {
		if self.ai_at != at {
			return None;
		}
		let repo = self.page.as_ref().map(|p| p.pr.repository.name.clone()).unwrap_or_default();
		let btn = |label: &str, pri: bool, m: Msg| html::button(label.to_owned()).class(if pri { "azbtn pri" } else { "azbtn" }).on_click(m);
		if let Some(dir) = &self.pick {
			let rows = self.pick_rows();
			let cur = self.pick_sel.min(rows.len().saturating_sub(1));
			let first = cur.saturating_sub(JUMP_ROWS - 1).min(rows.len().saturating_sub(JUMP_ROWS));
			let mut list = html::div().class("apl");
			for (i, (kind, d)) in rows.iter().enumerate().skip(first).take(JUMP_ROWS) {
				let (glyph, hint) = match kind {
					'u' => ("\u{2191}", "up"),
					'c' => ("\u{25b8}", "inside"),
					'f' => ("\u{25b8}", "match"),
					_ => ("\u{2194}", "beside"),
				};
				list = list.child(
					html::div()
						.class(if i == cur { "row cur" } else { "row" })
						.on_click(Msg::PickGo(d.path.clone()))
						.child(html::span(glyph).class("id"))
						.child(html::span(d.name.clone()).class("t"))
						.child(html::span(if d.git { format!("{hint} \u{b7} git") } else { hint.to_owned() }).class("pj")),
				);
			}
			if rows.is_empty() {
				list = list.child(html::div().class("row").child(html::span("No folders here.").class("dim")));
			}
			return Some(
				html::div()
					.key("aipop")
					.class("aipop")
					.child(html::div().class("apt").child(html::span(format!("Which folder is the {repo} clone?"))))
					.child(html::div().class("apd").child(html::span(review::show(dir))))
					.child(html::div().class("aps").child(html::span(if self.pick_q.is_empty() { "Type to search folders".to_owned() } else { format!("{}\u{2581}", self.pick_q) }).class(if self.pick_q.is_empty() { "dim" } else { "" })))
					.child(list)
					.child(
						html::div()
							.class("apf")
							.child(html::span(if self.pick_q.is_empty() { "click a folder or use the arrows; Enter uses the folder shown" } else { "Enter uses the highlighted folder; Esc clears the search" }).class("dim"))
							.child(btn("Cancel", false, Msg::PickCancel))
							.child(btn("Use this folder", true, Msg::PickUse)),
					),
			);
		}
		let c = self.compose.as_ref().filter(|c| matches!(c.kind, ComposeKind::ReviewPrompt(_) | ComposeKind::ReviewAgent))?;
		let mut panel = html::div().key("aipop").class("aipop");
		let prompt = matches!(c.kind, ComposeKind::ReviewPrompt(_));
		panel = panel.child(html::div().class("apt").child(html::span(match c.kind {
			ComposeKind::ReviewPrompt(k) => format!("The {} prompt for {repo}", k.label()),
			_ => format!("Agent for {repo}"),
		})));
		if prompt {
			panel = panel
				.child(html::div().class("dim aph").child(html::span(format!("Variables: {}", review::NAMES))))
				.child(html::div().class("apr").child(html::span(if self.preview { "Hide preview" } else { "Preview" }).class("pvt").on_click(Msg::Preview)));
			if self.preview {
				panel = panel.child(html::div().class("pvx").child(html::span(self.prompt_preview(&c.draft))));
			}
		} else {
			panel = panel.child(html::div().class("dim aph").child(html::span("claude, codex, or a command line; {prompt} marks where the prompt goes")));
		}
		let editor = ui::editor()
			.key("aied")
			.text(c.draft.clone())
			.cursor(c.utf16_cursor())
			.placeholder("")
			.sendable(true)
			.max_lines(if prompt { 10u32 } else { 2 })
			.on_send(|t| Msg::Send(t.to_owned()))
			.on_edit(|e| Msg::Edit(e.clone()))
			.on_focus(Msg::Focus);
		Some(
			panel.child(editor).child(
				html::div()
					.class("apf")
					.child(html::span(if prompt { "Enter saves \u{b7} Shift+Enter new line \u{b7} Ctrl+P preview" } else { "Enter saves" }).class("dim"))
					.child(btn("Cancel", false, Msg::Cancel))
					.child(btn("Save", true, Msg::Submit)),
			),
		)
	}

	/// The editor to focus: the one in the open panel, else the line's, else the dock's.
	fn focus_path(&self) -> Option<String> {
		let c = self.compose.as_ref()?;
		if matches!(c.kind, ComposeKind::ReviewPrompt(_) | ComposeKind::ReviewAgent) {
			let files = self.page.as_ref().is_some_and(|p| p.tab == PrTab::Files);
			return Some(match self.ai_at {
				AiAnchor::Header => "main.pbody.aipop.aied".to_owned(),
				AiAnchor::Thread(id) => format!("{}.t{id}.aipop.aied", if files { FILES_DS } else { OV_MAIN }),
				AiAnchor::Line => "main.pbody.fv.dp.ds.cbox.aipop.aied".to_owned(),
			});
		}
		Some(if matches!(c.kind, ComposeKind::Inline { .. }) { LINE_EDITOR } else { COMPOSER }.to_owned())
	}

	/// The picker's rows: the folder above, the folders inside, the folders beside it.
	fn pick_rows(&self) -> Vec<(char, review::Dir)> {
		let Some(dir) = &self.pick else { return Vec::new() };
		if !self.pick_q.trim().is_empty() {
			return review::search(dir, &self.pick_q).into_iter().map(|d| ('f', d)).collect();
		}
		let mut rows = Vec::new();
		if let Some(parent) = dir.parent() {
			let name = parent.file_name().map_or_else(|| review::show(parent), |n| n.to_string_lossy().into_owned());
			rows.push(('u', review::Dir { name: format!("..  {name}"), path: parent.to_path_buf(), git: parent.join(".git").exists() }));
			rows.extend(review::subdirs(dir).into_iter().take(60).map(|d| ('c', d)));
			rows.extend(review::subdirs(parent).into_iter().filter(|d| d.path != *dir).take(60).map(|d| ('s', d)));
		} else {
			rows.extend(review::subdirs(dir).into_iter().take(60).map(|d| ('c', d)));
		}
		rows
	}

	fn pick_key(&mut self, key: &Key) {
		let rows = self.pick_rows();
		let last = rows.len().saturating_sub(1);
		let searching = !self.pick_q.is_empty();
		let go = |app: &mut Self, path: std::path::PathBuf| {
			app.pick = Some(path);
			app.pick_sel = 0;
			app.pick_q.clear();
		};
		match key.name.as_str() {
			"escape" if searching => {
				self.pick_q.clear();
				self.pick_sel = 0;
			},
			"escape" => {
				self.pick = None;
				self.review_next = None;
			},
			// Searching: use the highlighted folder. Otherwise: the folder shown.
			"enter" if searching => {
				if let Some((_, d)) = rows.get(self.pick_sel.min(last)) {
					go(self, d.path.clone());
					self.pick_use();
				}
			},
			"enter" => self.pick_use(),
			"down" => self.pick_sel = (self.pick_sel + 1).min(last),
			"up" => self.pick_sel = self.pick_sel.saturating_sub(1),
			"page_down" => self.pick_sel = (self.pick_sel + JUMP_ROWS).min(last),
			"page_up" => self.pick_sel = self.pick_sel.saturating_sub(JUMP_ROWS),
			"right" | "tab" => {
				if let Some((_, d)) = rows.get(self.pick_sel.min(last)) {
					go(self, d.path.clone());
				}
			},
			"left" => {
				if let Some(parent) = self.pick.as_ref().and_then(|d| d.parent()).map(std::path::Path::to_path_buf) {
					go(self, parent);
				}
			},
			"backspace" if searching => {
				self.pick_q.pop();
				self.pick_sel = 0;
			},
			"backspace" => {
				if let Some(parent) = self.pick.as_ref().and_then(|d| d.parent()).map(std::path::Path::to_path_buf) {
					go(self, parent);
				}
			},
			_ => {
				if let Some(text) = key.typed().filter(|_| !key.ctrl && !key.alt) {
					self.pick_q.push_str(&text.to_owned());
					self.pick_sel = 0;
				}
			},
		}
	}

	/// Use the folder the picker shows, and go on with the request that was waiting for it.
	fn pick_use(&mut self) {
		let Some(dir) = self.pick.take() else { return };
		self.review.dir = review::show(&dir);
		if let Err(e) = config::save_review(&self.review_key, self.review.clone()) {
			self.note = format!("could not save the AI settings: {e}");
			return;
		}
		match self.review_next.take() {
			Some(req) => self.run_req(req),
			None => self.note = if dir.join(".git").exists() { "saved the checkout folder".into() } else { "saved (that folder is not a git checkout)".into() },
		}
	}

	/// Edit one kind of prompt for this repo, starting from what is saved or the default.
	fn edit_prompt(&mut self, kind: review::Kind) {
		let Some((key, r)) = self.review_settings() else { return };
		let text = r.prompts.get(kind.id()).filter(|t| !t.trim().is_empty()).cloned().unwrap_or_else(|| kind.default_prompt().to_owned());
		self.rv_menu = false;
		self.ai_at = AiAnchor::Header;
		self.compose = Some(Composer::new(ComposeKind::ReviewPrompt(kind), text));
		self.review = r;
		self.review_key = key;
	}

	/// Start the agent on the PR or a comment thread in a new block, in this repo's checkout.
	fn run_ai(&mut self, kind: review::Kind, thread: Option<u32>) {
		self.run_req(AiReq { kind, thread, line: None });
	}

	/// Start the agent for `req`. A repo with no checkout yet gets one found from where the plugin started, else the picker.
	fn run_req(&mut self, req: AiReq) {
		self.rv_menu = false;
		let Some((key, mut r)) = self.review_settings() else { return };
		if matches!(req.kind, review::Kind::Explain | review::Kind::Respond) && self.review_vars(req.thread, None).is_some_and(|v| v.thread == "(no comment)") {
			self.note = "this pull request has no comments yet".into();
			return;
		}
		if !std::path::Path::new(r.dir.trim()).is_dir() {
			let repo = self.page.as_ref().map(|p| p.pr.repository.name.clone()).unwrap_or_default();
			match review::find_checkout(&repo) {
				Some(dir) => {
					r.dir = dir;
					if let Err(e) = config::save_review(&key, r.clone()) {
						self.note = format!("could not save the AI settings: {e}");
					}
				},
				None => {
					self.review_setup(Some(req));
					return;
				},
			}
		}
		let Some(vars) = self.review_vars(req.thread, req.line.as_ref()) else { return };
		let template = r.prompts.get(req.kind.id()).filter(|t| !t.trim().is_empty()).map_or(req.kind.default_prompt(), String::as_str);
		let prompt = review::render(template, &vars);
		self.note = match review::launch(r.dir.trim(), &review::argv(&r.agent, &prompt)) {
			Ok(m) => m,
			Err(e) => format!("AI request failed: {e}"),
		};
	}

	/// The prompt being edited with its variables filled in.
	fn prompt_preview(&self, draft: &str) -> String {
		self.review_vars(None, None).map(|v| review::render(draft, &v)).unwrap_or_default()
	}

	fn send(&mut self, text: String) {
		let Some(c) = self.compose.take() else { return };
		match c.kind {
			ComposeKind::ReviewAgent => {
				self.review.agent = if text.trim().is_empty() { review::DEFAULT_AGENT.into() } else { text.trim().to_owned() };
				self.note = match config::save_review(&self.review_key, self.review.clone()) {
					Ok(()) => "saved the agent".into(),
					Err(e) => format!("could not save the AI settings: {e}"),
				};
			},
			ComposeKind::ReviewPrompt(kind) => {
				// The default is not saved, so a better default reaches everyone who never changed it.
				if text.trim().is_empty() || text.trim() == kind.default_prompt() {
					self.review.prompts.remove(kind.id());
				} else {
					self.review.prompts.insert(kind.id().to_owned(), text.trim_end().to_owned());
				}
				self.note = match config::save_review(&self.review_key, self.review.clone()) {
					Ok(()) => format!("saved the {} prompt", kind.label()),
					Err(e) => format!("could not save the prompt: {e}"),
				};
			},
			ComposeKind::Org => match config::normalize(&text) {
				Some(org) => {
					if let Err(e) = config::save_org(&org) {
						self.note = format!("could not save the config: {e}");
					}
					self.connect(&org);
				},
				None => {
					self.compose = Some(Composer::new(ComposeKind::Org, text));
					self.note = "that does not look like an Azure DevOps organization".into();
				},
			},
			ComposeKind::Jump => {
				let ids = self.jump_results(&text);
				if let Some(id) = ids.get(self.jump.min(ids.len().saturating_sub(1))).copied() {
					self.open(id);
				}
			},
			ComposeKind::Filter => {
				self.filter = text.trim().to_owned();
				self.fix_selection();
			},
			ComposeKind::Comment(thread) => {
				if text.trim().is_empty() {
					return;
				}
				if let Some(p) = &self.page {
					let pr = p.pr.clone();
					self.note = "sending…".into();
					self.start(Job::Comment { pr, thread, text: text.trim_end().to_owned() });
				}
			},
			ComposeKind::Line => {
				let t = text.trim();
				let (left, num) = match t.strip_prefix('-') {
					Some(n) => (true, n.trim()),
					None => (false, t.trim_start_matches('+').trim()),
				};
				match num.parse::<u32>().ok().filter(|n| self.page.as_ref().is_some_and(|p| p.has_line(*n, left))) {
					Some(line) => self.compose = Some(Composer::new(ComposeKind::Inline { line, left }, String::new())),
					None => {
						self.note = format!("this file has no {}line {t}", if left { "old " } else { "" });
						self.compose = Some(Composer::new(ComposeKind::Line, text));
					},
				}
			},
			ComposeKind::Inline { line, left } => {
				if text.trim().is_empty() {
					self.compose = Some(Composer::new(c.kind, text));
					return;
				}
				if let Some(p) = &self.page
					&& let Some(c) = p.change()
				{
					let at = api::LineRef {
						path: c.path().to_owned(),
						line,
						left,
						iteration: p.detail.as_ref().map_or(1, |d| d.iteration),
						tracking: c.change_tracking_id,
					};
					let job = Job::InlineComment { pr: p.pr.clone(), at, text: text.trim_end().to_owned() };
					self.note = "sending…".into();
					self.start(job);
				}
			},
		}
	}

	/// A key while the composer is up. True when it closed.
	fn compose_key(&mut self, key: &Key) -> bool {
		let Some(kind) = self.compose.as_ref().map(|c| c.kind) else { return false };
		if key.is("escape") {
			if kind == ComposeKind::Filter {
				self.filter.clear();
				self.fix_selection();
			}
			// Without an organization there is nothing to go back to.
			if kind == ComposeKind::Org && self.client.is_none() {
				return false;
			}
			self.compose = None;
			return true;
		}
		if kind == ComposeKind::Jump {
			let n = self.compose.as_ref().map_or(0, |c| self.jump_results(&c.draft).len());
			let last = n.saturating_sub(1);
			match key.name.as_str() {
				"down" | "tab" if !key.shift => self.jump = (self.jump + 1).min(last),
				"up" | "tab" => self.jump = self.jump.saturating_sub(1),
				"n" if key.ctrl => self.jump = (self.jump + 1).min(last),
				"p" if key.ctrl => self.jump = self.jump.saturating_sub(1),
				"page_down" => self.jump = (self.jump + JUMP_ROWS).min(last),
				"page_up" => self.jump = self.jump.saturating_sub(JUMP_ROWS),
				_ => {
					let Some(c) = &mut self.compose else { return false };
					if key.name == "enter" {
						let text = std::mem::take(&mut c.draft);
						self.send(text);
						return true;
					}
					c.key(key);
					self.jump = 0;
					return false;
				},
			}
			return true;
		}
		if matches!(kind, ComposeKind::ReviewPrompt(_)) && key.ctrl && key.name == "p" {
			self.preview = !self.preview;
			return true;
		}
		let Some(c) = &mut self.compose else { return false };
		let multiline = matches!(kind, ComposeKind::Comment(_) | ComposeKind::Inline { .. } | ComposeKind::ReviewPrompt(_));
		if key.name == "enter" {
			if multiline && (key.shift || key.alt) {
				c.insert("\n");
				return false;
			}
			let text = std::mem::take(&mut c.draft);
			self.send(text);
			return true;
		}
		c.key(key);
		if c.kind == ComposeKind::Filter {
			self.filter = c.draft.clone();
			self.fix_selection();
		}
		false
	}

	/// A key with nothing else taking it. False to quit.
	fn key(&mut self, key: &Key) -> bool {
		if (self.menu.is_some() || self.rv_menu) && key.is("escape") {
			self.menu = None;
			self.rv_menu = false;
			return true;
		}
		if self.voting {
			self.voting = false;
			let vote = match key.name.as_str() {
				"1" => Some(10),
				"2" => Some(5),
				"3" => Some(-5),
				"4" => Some(-10),
				"0" => Some(0),
				_ => None,
			};
			if let (Some(v), Some(p), Some(me)) = (vote, &self.page, &self.me) {
				let job = Job::Vote { pr: p.pr.clone(), me: me.id.clone(), vote: v };
				self.note = format!("voting {}…", api::vote_label(v));
				self.start(job);
			}
			return true;
		}
		if key.is("ctrl+q") {
			return false;
		}
		let plain_slash = key.name == "/" && !key.ctrl && !key.alt;
		if plain_slash || key.is("ctrl+k") {
			self.jump = 0;
			self.compose = Some(Composer::new(ComposeKind::Jump, String::new()));
			return true;
		}
		if key.is("ctrl+f") && self.page.is_none() {
			self.compose = Some(Composer::new(ComposeKind::Filter, self.filter.clone()));
			return true;
		}
		// Vim motions: gg and G, Ctrl+D / Ctrl+U (or d / u, as in less) for half a screen, Ctrl+F / Ctrl+B for a screen.
		let after_g = std::mem::take(&mut self.g_pending);
		let plain = !key.ctrl && !key.alt && !key.meta;
		let ctl = key.ctrl && !key.alt;
		let shift_g = plain && (key.name == "G" || (key.name == "g" && key.shift));
		if plain && key.name == "g" && !key.shift {
			if after_g {
				self.motion(Motion::Top);
			} else {
				self.g_pending = true;
			}
			return true;
		}
		let motion = match key.name.as_str() {
			_ if shift_g => Some(Motion::Bottom),
			"d" if plain || ctl => Some(Motion::Screen(0.5)),
			"u" if plain || ctl => Some(Motion::Screen(-0.5)),
			"f" if ctl => Some(Motion::Screen(1.0)),
			"b" if ctl => Some(Motion::Screen(-1.0)),
			_ => None,
		};
		if let Some(m) = motion {
			self.motion(m);
			return true;
		}
		let url = || self.selected_pr().and_then(|p| Some(self.client.as_ref()?.web_url(p)));
		match key.name.as_str() {
			"r" if !key.ctrl => self.refresh(),
			"o" => {
				if let Some(u) = url() {
					self.note = match open_url(&u) {
						Ok(()) => "opened in the browser".into(),
						Err(e) => format!("could not open the browser: {e}"),
					};
				}
			},
			"y" => {
				if let Some(u) = url() {
					self.copy(&u, "link");
				}
			},
			"b" => {
				if let Some(b) = self.selected_pr().map(|p| p.source().to_owned()) {
					self.copy(&b, "branch name");
				}
			},
			_ if self.page.is_some() => self.page_key(key),
			_ => return self.list_key(key),
		}
		true
	}

	/// A node inside the scroll container on screen: the overview's, or the diff's when a file shows.
	fn scroll_anchor(&self) -> Option<&'static str> {
		let p = self.page.as_ref()?;
		match p.tab {
			PrTab::Overview => p.detail.is_some().then_some(OV_FIRST),
			PrTab::Files => p.change().is_some().then_some(FILES_FIRST),
		}
	}

	/// Moves the list selection, the overview or the diff by a vim motion.
	fn motion(&mut self, m: Motion) {
		if self.page.is_none() {
			let cap = self.rows_in(self.list_lines()) as f64;
			match m {
				Motion::Top => self.move_sel(i64::MIN / 2),
				Motion::Bottom => self.move_sel(i64::MAX / 2),
				Motion::Screen(f) => self.move_sel(((cap * f.abs()).max(1.0) as i64) * f.signum() as i64),
			}
			return;
		}
		match m {
			Motion::Top => self.scroll(i64::MIN / 2),
			Motion::Bottom => self.scroll(i64::MAX / 2),
			// One Tern line step is about one text line, so a screen is the body's lines.
			Motion::Screen(f) => self.scroll_req = Some(ScrollReq::Lines((self.page_body_lines() * 0.9 * f) as i64)),
		}
	}

	fn list_key(&mut self, key: &Key) -> bool {
		let cap = self.rows_in(self.list_lines()) as i64;
		match key.name.as_str() {
			"q" => return false,
			"down" | "j" => self.move_sel(1),
			"up" | "k" => self.move_sel(-1),
			"page_down" => self.move_sel(cap - 2),
			"page_up" => self.move_sel(-(cap - 2)),
			"home" => self.move_sel(i64::MIN / 2),
			"end" => self.move_sel(i64::MAX / 2),
			"enter" => {
				if let Some(id) = self.sel {
					self.open(id);
				}
			},
			"1" | "2" | "3" => {
				self.tab = ListTab::ALL[key.name.parse::<usize>().unwrap_or(1) - 1];
				self.fix_selection();
			},
			"tab" => {
				let i = ListTab::ALL.iter().position(|t| *t == self.tab).unwrap_or(0);
				self.tab = ListTab::ALL[(i + if key.shift { 2 } else { 1 }) % 3];
				self.fix_selection();
			},
			// Tern can keep `/` for itself, so `f` and Ctrl+F open the filter too.
			"f" => self.compose = Some(Composer::new(ComposeKind::Filter, self.filter.clone())),
			"escape" if !self.filter.is_empty() => {
				self.filter.clear();
				self.fix_selection();
			},
			_ => {},
		}
		true
	}

	fn page_key(&mut self, key: &Key) {
		let Some(p) = &mut self.page else { return };
		let files = p.tab == PrTab::Files;
		match key.name.as_str() {
			"escape" | "backspace" | "q" => {
				if p.thread.is_some() {
					p.thread = None;
				} else if self.cols < RAIL_MIN_COLS {
					// Narrow: back to the navigation rail. Wide: the rail is always there.
					self.page = None;
					self.fix_selection();
				}
			},
			// Vim's next/previous change; Shift+] and Shift+[ may arrive as the unshifted key.
			"}" => self.step_change(true),
			"{" => self.step_change(false),
			"]" if key.shift => self.step_change(true),
			"[" if key.shift => self.step_change(false),
			"]" => self.step_pr(true),
			"[" => self.step_pr(false),
			"tab" | "1" | "2" => {
				p.tab = match key.name.as_str() {
					"1" => PrTab::Overview,
					"2" => PrTab::Files,
					_ if files => PrTab::Overview,
					_ => PrTab::Files,
				};
				p.thread = None;
				if p.tab == PrTab::Files {
					self.load_file();
				}
			},
			"down" | "j" if files => self.step_file(true),
			"up" | "k" if files => self.step_file(false),
			"h" | "left" if files => self.fold_here(true),
			"l" | "right" if files => self.fold_here(false),
			"s" if files => p.split = !p.split,
			"down" | "j" => self.scroll(1),
			"up" | "k" => self.scroll(-1),
			"page_down" | "space" | "f" => self.motion(Motion::Screen(1.0)),
			"page_up" => self.motion(Motion::Screen(-1.0)),
			"home" => self.scroll(i64::MIN / 2),
			"end" => self.scroll(i64::MAX / 2),
			"n" => self.step_thread(!key.shift),
			"p" | "N" => self.step_thread(false),
			"x" if files => {
				p.full = !p.full;
				self.rebuild_diffs();
			},
			"v" => self.voting = true,
			"I" => self.rv_menu = !self.rv_menu,
			"i" if key.shift => self.rv_menu = !self.rv_menu,
			"i" => self.run_ai(review::Kind::Review, None),
			"t" => self.run_ai(review::Kind::Context, None),
			"e" | "w" => match p.thread {
				Some(t) => self.run_ai(if key.name == "e" { review::Kind::Explain } else { review::Kind::Respond }, Some(t)),
				None => self.note = "pick a comment first (n / p, or click one)".into(),
			},
			"c" if files => {
				if p.file_ready() {
					self.compose = Some(Composer::new(ComposeKind::Line, String::new()));
				} else {
					self.note = "the file is still loading".into();
				}
			},
			"c" => self.compose = Some(Composer::new(ComposeKind::Comment(None), String::new())),
			"a" => match p.thread {
				Some(t) => self.compose = Some(Composer::new(ComposeKind::Comment(Some(t)), String::new())),
				None => self.note = "pick a comment first (n / p, or click one)".into(),
			},
			_ => {},
		}
	}

	fn msg(&mut self, msg: Msg) {
		match msg {
			Msg::Sort(id) => {
				self.menu = None;
				if let Some(m) = SortMode::ALL.into_iter().find(|m| m.id() == id) {
					self.sort = m;
				}
			},
			Msg::Menu(m) => self.menu = if self.menu == Some(m) { None } else { Some(m) },
			Msg::Project(n) => {
				self.menu = None;
				self.project = n;
				self.fix_selection();
			},
			Msg::Status(id) => {
				self.menu = None;
				if let Some(f) = StatusFilter::ALL.into_iter().find(|f| f.id() == id) {
					self.status = f;
					self.fix_selection();
				}
			},
			Msg::Open(id) => self.open(id),
			Msg::ListTab(id) => {
				self.menu = None;
				if let Some(t) = ListTab::ALL.into_iter().find(|t| t.id() == id) {
					self.tab = t;
					self.fix_selection();
				}
			},
			Msg::PrTab(id) => {
				if let Some(p) = &mut self.page {
					p.tab = if id == "files" { PrTab::Files } else { PrTab::Overview };
					p.thread = None;
				}
				self.load_file();
			},
			Msg::File(i) => self.pick_file(i),
			Msg::Folder(path) => {
				if let Some(p) = &mut self.page
					&& !p.folded.remove(&path)
				{
					p.folded.insert(path);
				}
			},
			Msg::Split(on) => {
				if let Some(p) = &mut self.page {
					p.split = on;
				}
			},
			Msg::Full(on) => {
				if let Some(p) = &mut self.page
					&& p.full != on
				{
					p.full = on;
					self.rebuild_diffs();
				}
			},
			Msg::Thread(t) => {
				if let Some(p) = &mut self.page {
					p.thread = if p.thread == Some(t) { None } else { Some(t) };
				}
			},
			Msg::Send(text) => self.send(text),
			Msg::LineAt(line, left) => {
				if self.page.as_ref().is_some_and(|p| p.tab == PrTab::Files && p.file_ready()) {
					// Moving the box keeps what was typed.
					let draft = self.compose.take().filter(|c| matches!(c.kind, ComposeKind::Inline { .. })).map(|c| c.draft).unwrap_or_default();
					self.compose = Some(Composer::new(ComposeKind::Inline { line, left }, draft));
					self.tree_req = Some(LINE_BOX.to_owned());
				}
			},
			Msg::Cancel => self.compose = None,
			Msg::Change(forward) => self.step_change(forward),
			Msg::Ai(kind, thread) => self.run_ai(kind, thread),
			Msg::ReviewMenu => self.rv_menu = !self.rv_menu,
			Msg::EditPrompt(kind) => self.edit_prompt(kind),
			Msg::ReviewSetup => self.review_setup(None),
			Msg::ReviewAgent => {
				if let Some((key, r)) = self.review_settings() {
					self.rv_menu = false;
					self.ai_at = AiAnchor::Header;
					self.compose = Some(Composer::new(ComposeKind::ReviewAgent, r.agent.clone()));
					self.review = r;
					self.review_key = key;
				}
			},
			Msg::AiLine(kind) => {
				let req = self.compose.as_ref().and_then(|c| match c.kind {
					ComposeKind::Inline { line, left } => Some(AiReq { kind, thread: None, line: Some((line, left, c.draft.clone())) }),
					_ => None,
				});
				if let Some(req) = req {
					self.run_req(req);
				}
			},
			Msg::PickGo(path) => {
				self.pick = Some(path);
				self.pick_sel = 0;
				self.pick_q.clear();
			},
			Msg::PickUse => self.pick_use(),
			Msg::PickCancel => {
				self.pick_q.clear();
				self.pick = None;
				self.review_next = None;
			},
			Msg::Preview => self.preview = !self.preview,
			Msg::Submit => {
				if let Some(text) = self.compose.as_ref().map(|c| c.draft.clone()) {
					self.send(text);
				}
			},
			Msg::Edit(e) => {
				if let Some(c) = &mut self.compose {
					if !c.apply(&e) {
						self.note = "the comment box got out of step; press Esc and start again".into();
					}
					match c.kind {
						ComposeKind::Filter => {
							self.filter = c.draft.clone();
							self.fix_selection();
						},
						ComposeKind::Jump => self.jump = 0,
						_ => {},
					}
				}
			},
			Msg::Focus => {},
		}
	}
}

/// Tern can report Ctrl as meta; off macOS (where meta is Cmd) treat it as Ctrl so Ctrl shortcuts work.
fn ctrl_from_meta(mut key: Key) -> Key {
	if !cfg!(target_os = "macos") && key.meta && !key.ctrl {
		key.meta = false;
		key.ctrl = true;
	}
	key
}

pub fn run(org: Option<String>, demo: bool) -> anyhow::Result<()> {
	let Some(mut session) = Session::<Msg>::connect(Options::new().feature(feature::SEND).feature(feature::EDIT))? else {
		anyhow::bail!("the interactive view needs Tern; use `tern-azdo list` elsewhere");
	};
	let sf = session.open(SurfaceOptions::screen().title("Pull Requests").role("azdo"))?;
	session.stylesheet(sf, "azdo", Some(CSS))?;
	let mut app = App::new(org, demo);
	let mut focused: Option<String> = None;
	let mut rows_checked = Instant::now();
	// TERN_AZDO_TRACE=file logs every input, for seeing what Tern sends.
	let mut trace = std::env::var_os("TERN_AZDO_TRACE").and_then(|p| std::fs::File::create(p).ok());
	loop {
		session.render(sf, app.view())?;
		if let Some(req) = app.scroll_req.take()
			&& let Some(anchor) = app.scroll_anchor()
		{
			let done = match req {
				ScrollReq::Lines(n) => {
					let by = if n < 0 { ScrollBy::LineUp } else { ScrollBy::LineDown };
					(0..n.unsigned_abs()).try_for_each(|_| session.scroll(sf, anchor, by))
				},
				ScrollReq::By(by) => session.scroll(sf, anchor, by),
				ScrollReq::Reveal(id) => session.reveal(sf, &id, RevealAt::Start),
			};
			if let Err(e) = done {
				app.note = format!("could not scroll: {e}");
			}
		}
		if let Some(id) = app.tree_req.take()
			&& let Err(e) = session.reveal(sf, &id, RevealAt::Nearest)
		{
			app.note = format!("could not scroll the file list: {e}");
		}
		// After the render, so the composer exists when it takes focus.
		let want_focus = app.focus_path();
		if want_focus != focused {
			session.focus(sf, want_focus.as_deref())?;
			focused = want_focus;
		}
		let wait = Duration::from_millis(if app.busy > 0 { 100 } else { 1000 });
		let input = loop {
			match session.next(Some(wait)) {
				Ok(Some(i)) => break Some(i),
				Ok(None) => {
					if app.poll() {
						break None;
					}
					if rows_checked.elapsed() > Duration::from_millis(1500) {
						rows_checked = Instant::now();
						if let Some(r) = pane_rows().filter(|r| (*r - app.rows).abs() > 0.1) {
							app.rows = r;
							break None;
						}
					}
				},
				Err(tern_sdk::Error::InputClosed) => return session.close().map_err(Into::into),
				Err(e) => return Err(e.into()),
			}
		};
		app.poll();
		let Some(input) = input else { continue };
		if let Some(log) = &mut trace {
			let _ = std::io::Write::write_all(log, format!("{input:?}
").as_bytes());
		}
		match input {
			Input::Key(key) => {
				let key = ctrl_from_meta(key);
				// A note lasts until the next key.
				app.note.clear();
				if app.pick.is_some() {
					app.pick_key(&key);
				} else if app.compose.is_some() {
					app.compose_key(&key);
				} else if !app.key(&key) {
					break;
				}
			},
			Input::Msg(m, _) => app.msg(m),
			Input::Event(Event::Resize(r)) => {
				app.rows = pane_rows().unwrap_or(app.rows);
				if let Some(c) = r.cell {
					app.cell_h = c.h;
				}
				app.cols = f64::from(r.cols);
			},
			_ => {},
		}
	}
	session.close()?;
	Ok(())
}
