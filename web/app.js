(() => {
  const $ = (id) => document.getElementById(id);
  const svg = $('canvas');
  let doc, layerId, tool = 'pen', draft = [], selected = null, dragging = null, toastTimer, hover = null;
  const undo = [], redo = [];
  let sharedBase = null;
  let geometryBusy=false;
  let editGeometry=null, geometryDrag=null;
  async function geometryOperation(operation,id=selected){
    if(geometryBusy)return null;
    const layer=id?doc.layers.find(l=>l.paths.some(p=>p.id===id)):activeLayer();
    if(!layer || (id===null && $('geometryTarget').value!=='layer')){notify('Select a path first',true);return null;}
    const baseDocument=JSON.stringify(doc);
    geometryBusy=true;
    try{const r=await fetch('/api/geometry',{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({document:doc,layer:layer.id,id,operation})});const data=await r.json();if(!r.ok)throw new Error(data.error);if(JSON.stringify(doc)!==baseDocument)throw new Error('Artwork changed while calculating. Try the operation again.');if(!['bounds','nodes','hit'].includes(operation.type)){remember();doc=data.document;editGeometry=null;$('nodeEditor').replaceChildren();render();const p=selectedPath();if(p)$('pathData').value=p.d;}return data.result;}catch(e){notify(e.message,true);return null;}finally{geometryBusy=false;}
  }
  const targetId=()=>$('geometryTarget').value==='layer'?null:selected;
  $('moveGeometry').onclick=()=>geometryOperation({type:'translate',dx:Number($('moveX').value),dy:Number($('moveY').value)},targetId());
  $('rotateGeometry').onclick=()=>geometryOperation({type:'rotate',degrees:Number($('rotateDegrees').value),cx:0,cy:0},targetId());
  $('scaleGeometry').onclick=()=>geometryOperation({type:'scale',sx:Number($('scaleX').value),sy:Number($('scaleY').value),cx:0,cy:0},targetId());
  $('editNodes').onclick=async()=>{
    if(!selected){notify('Select a path first',true);return;}
    const result=await geometryOperation({type:'nodes'});if(!result)return;
    editGeometry={id:selected,nodes:result.nodes};render();
    const root=$('nodeEditor');root.replaceChildren();
    for(const node of result.nodes){
      function row(label,x,y,operation){const div=document.createElement('div');const title=document.createElement('span');title.textContent=label;const ix=document.createElement('input'),iy=document.createElement('input');for(const input of [ix,iy]){input.type='number';input.step='0.1';input.style.width='72px';}ix.value=x;iy.value=y;const apply=document.createElement('button');apply.textContent='Apply';apply.onclick=()=>geometryOperation({...operation,x:Number(ix.value),y:Number(iy.value)});div.append(title,ix,iy,apply);root.append(div);}
      row(`Anchor ${node.index}`,node.x,node.y,{type:'move-anchor',index:node.index});
      for(const h of node.handles)row(`Handle ${node.element}.${h.handle}`,h.x,h.y,{type:'set-handle',element:node.element,handle:h.handle});
    }
  };
  async function loadShared(){try{const r=await fetch('/api/document');if(!r.ok)return;const data=await r.json();sharedBase=data.document;doc=structuredClone(sharedBase);layerId=doc.layers[0]?.id;draft=[];selected=null;undo.length=0;redo.length=0;render();$('saveBtn').textContent='Save shared';$('reloadBtn').hidden=false;notify('Shared document loaded');}catch(e){notify(`Load failed: ${e.message}`,true)}}
  const snapshot = () => JSON.stringify({doc,layerId,draft,selected});
  function remember(){undo.push(snapshot());redo.length=0;}
  function restore(from,to){if(!from.length)return;to.push(snapshot());({doc,layerId,draft,selected}=JSON.parse(from.pop()));dragging=null;hover=null;editGeometry=null;$('nodeEditor').replaceChildren();$('pathData').value=selectedPath()?.d||'';render();}
  const uid = (prefix) => `${prefix}-${crypto.randomUUID ? crypto.randomUUID() : Date.now().toString(36)}`;
  const fresh = () => ({format:'pentool',version:1,name:'Untitled',canvas:{width:1200,height:800,background:'#ffffff'},layers:[{id:uid('layer'),name:'Layer 1',visible:true,locked:false,paths:[]}]});
  const activeLayer = () => doc.layers.find(l => l.id === layerId) || doc.layers[0];
  const ns = 'http://www.w3.org/2000/svg';

  function notify(message, error=false) { const t=$('toast'); t.textContent=message; t.className=`toast show${error?' error':''}`; clearTimeout(toastTimer); toastTimer=setTimeout(()=>t.className='toast',2600); }
  function escapeXml(s) { return String(s).replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&apos;'}[c])); }
  function render() {
    svg.setAttribute('viewBox',`0 0 ${doc.canvas.width} ${doc.canvas.height}`);
    const maxW=Math.max(280,document.querySelector('.stage').clientWidth-80), maxH=Math.max(240,document.querySelector('.stage').clientHeight-80);
    const scale=Math.min(maxW/doc.canvas.width,maxH/doc.canvas.height,1);
    svg.style.width=`${doc.canvas.width*scale}px`; svg.style.height=`${doc.canvas.height*scale}px`; svg.style.background=doc.canvas.background;
    svg.replaceChildren();
    doc.layers.filter(l=>l.visible).forEach(layer=>layer.paths.forEach(p=>{
      const el=document.createElementNS(ns,'path');
      for(const [k,v] of Object.entries({d:p.d,stroke:p.stroke,'stroke-width':p.stroke_width,fill:p.fill,'stroke-linecap':'round','stroke-linejoin':'round'})) el.setAttribute(k,v);
      el.dataset.id=p.id; if(p.id===selected) el.classList.add('selected'); svg.append(el);
    }));
    if(draft.length){const p=document.createElementNS(ns,'path');p.setAttribute('d',pathData(draft));p.setAttribute('stroke',$('stroke').value);p.setAttribute('stroke-width',$('width').value);p.setAttribute('fill','none');p.setAttribute('stroke-dasharray','6 5');p.setAttribute('stroke-linecap','round');p.setAttribute('pointer-events','none');svg.append(p);}
    if(draft.length){
      const overlay=document.createElementNS(ns,'g');overlay.setAttribute('pointer-events','none');
      const size=5/scale;
      function shape(tag,attrs){const el=document.createElementNS(ns,tag);for(const [key,value] of Object.entries(attrs))el.setAttribute(key,value);overlay.append(el);}
      if(hover&&!dragging){shape('path',{d:pathData([...draft,hover]),fill:'none',stroke:'#2563eb','stroke-width':1/scale,'stroke-dasharray':`${4/scale} ${4/scale}`});}
      draft.forEach((p,i)=>{
        for(const handle of [p.in,p.out].filter(Boolean)){shape('line',{x1:p.x,y1:p.y,x2:handle.x,y2:handle.y,stroke:'#2563eb','stroke-width':1/scale});shape('circle',{cx:handle.x,cy:handle.y,r:3/scale,fill:'white',stroke:'#2563eb','stroke-width':1/scale});}
        shape('rect',{x:p.x-size,y:p.y-size,width:size*2,height:size*2,fill:i===draft.length-1?'#2563eb':'white',stroke:'#2563eb','stroke-width':1/scale});
      });svg.append(overlay);
    }
    svg.style.cursor=tool==='pen'?'crosshair':tool==='hand'?'grab':'default';
    if(editGeometry?.id===selected&&tool==='select'){
      const size=5/scale;
      function marker(tag,attrs,data){const el=document.createElementNS(ns,tag);for(const [k,v] of Object.entries(attrs))el.setAttribute(k,v);Object.assign(el.dataset,data||{});svg.append(el);}
      for(const node of editGeometry.nodes){
        for(const h of node.handles){marker('line',{x1:node.x,y1:node.y,x2:h.x,y2:h.y,stroke:'#2563eb','stroke-width':1/scale,'pointer-events':'none'});marker('circle',{cx:h.x,cy:h.y,r:size,fill:'white',stroke:'#2563eb','stroke-width':1/scale},{geometry:'handle',element:node.element,handle:h.handle});}
        marker('rect',{x:node.x-size,y:node.y-size,width:size*2,height:size*2,fill:'white',stroke:'#2563eb','stroke-width':1/scale},{geometry:'anchor',index:node.index});
      }
    }
    renderLayers();
  }
  const n = value => value.toFixed(2);
  const pathData = points => points.map((p,i)=>{
    if(!i) return `M ${n(p.x)} ${n(p.y)}`;
    const prev=points[i-1], a=prev.out||prev, b=p.in||p;
    return `C ${n(a.x)} ${n(a.y)} ${n(b.x)} ${n(b.y)} ${n(p.x)} ${n(p.y)}`;
  }).join(' ');
  function point(e){const r=svg.getBoundingClientRect();return{x:(e.clientX-r.left)*doc.canvas.width/r.width,y:(e.clientY-r.top)*doc.canvas.height/r.height};}
  function finish(closed=false){if(draft.length>1){remember();const points=closed?[...draft,draft[0]]:draft;activeLayer().paths.push({id:uid('path'),d:pathData(points)+(closed?' Z':''),stroke:$('stroke').value,stroke_width:Number($('width').value),fill:$('fill').value,closed});notify(closed?'Closed path added':'Path added');}draft=[];hover=null;dragging=null;render();}
  function constrain(p,origin){const dx=p.x-origin.x,dy=p.y-origin.y,length=Math.hypot(dx,dy),angle=Math.round(Math.atan2(dy,dx)/(Math.PI/4))*Math.PI/4;return{x:origin.x+Math.cos(angle)*length,y:origin.y+Math.sin(angle)*length};}
  function near(a,b){return Math.hypot(a.x-b.x,a.y-b.y)<10*doc.canvas.width/svg.getBoundingClientRect().width;}
  function renderLayers(){const root=$('layers');root.replaceChildren();[...doc.layers].reverse().forEach(layer=>{const row=document.createElement('div');row.className=`layer${layer.id===layerId?' active':''}`;const eye=document.createElement('button');eye.setAttribute('aria-label',`${layer.visible?'Hide':'Show'} ${layer.name}`);eye.textContent=layer.visible?'◉':'○';eye.onclick=()=>{layer.visible=!layer.visible;render()};const name=document.createElement('button');name.className='layer-name';name.textContent=layer.name;name.onclick=()=>{layerId=layer.id;render()};const lock=document.createElement('button');lock.setAttribute('aria-label',`${layer.locked?'Unlock':'Lock'} ${layer.name}`);lock.textContent=layer.locked?'🔒':'·';lock.onclick=()=>{layer.locked=!layer.locked;render()};row.append(eye,name,lock);root.append(row);});}
  function download(data,name,type){const a=document.createElement('a');a.href=URL.createObjectURL(new Blob([data],{type}));a.download=name;a.click();setTimeout(()=>URL.revokeObjectURL(a.href),1000);}
  async function exportPng(){const btn=$('exportBtn');btn.disabled=true;btn.setAttribute('aria-busy','true');try{const r=await fetch('/api/render/png',{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify(doc)});if(!r.ok){const j=await r.json();throw new Error(j.error)}download(await r.blob(),`${doc.name || 'artwork'}.png`,'image/png');notify('PNG exported');}catch(e){notify(`Export failed: ${e.message}`,true)}finally{btn.disabled=false;btn.removeAttribute('aria-busy')}}
  svg.addEventListener('pointerdown',e=>{if(e.button!==0)return;if(tool==='pen'&&!activeLayer().locked&&activeLayer().visible){const p=point(e);if(draft.length>1&&near(p,draft[0])){finish(true);return;}remember();dragging={index:draft.length,start:p};draft.push(e.shiftKey&&draft.length?constrain(p,draft[draft.length-1]):p);hover=null;svg.setPointerCapture(e.pointerId);render()}else if(tool==='select'){const l=doc.layers.find(l=>l.paths.some(p=>p.id===e.target.dataset.id));selected=l&&!l.locked?e.target.dataset.id:null;const p=l?.paths.find(p=>p.id===selected);$('pathData').value=p?.d||'';if(p){layerId=l.id;$('stroke').value=p.stroke;$('width').value=p.stroke_width;$('widthOut').textContent=`${p.stroke_width} px`;}render()}});
  svg.addEventListener('pointermove',e=>{if(!dragging)return;const p=point(e), anchor=draft[dragging.index], dx=p.x-dragging.start.x,dy=p.y-dragging.start.y;anchor.out={x:anchor.x+dx,y:anchor.y+dy};anchor.in={x:anchor.x-dx,y:anchor.y-dy};render()});
  svg.addEventListener('pointerup',e=>{if(dragging){dragging=null;svg.releasePointerCapture(e.pointerId)}});
  svg.addEventListener('dblclick',e=>{e.preventDefault();finish()});
  document.addEventListener('keydown',e=>{if(/input|select|textarea/i.test(e.target.tagName))return;if((e.key==='Enter')&&draft.length){finish();e.preventDefault()}if(e.key==='Escape'){draft=[];selected=null;editGeometry=null;render()}if((e.key==='Delete'||e.key==='Backspace')&&selectedPath()){remember();for(const l of doc.layers)l.paths=l.paths.filter(p=>p.id!==selected);selected=null;editGeometry=null;render();e.preventDefault()}if(e.key.toLowerCase()==='p')setTool('pen');if(e.key.toLowerCase()==='v')setTool('select');if((e.ctrlKey||e.metaKey)&&e.key.toLowerCase()==='s'){e.preventDefault();save();}});
  function setTool(value){tool=value;document.querySelectorAll('.tool').forEach(b=>{const on=b.dataset.tool===tool;b.classList.toggle('active',on);b.setAttribute('aria-pressed',on)});$('hint').textContent=tool==='pen'?'Click to place points · Enter to finish · Esc to cancel':tool==='select'?'Click a path · Delete to remove':'Pan tool is reserved for the next release';}
  document.querySelectorAll('.tool').forEach(b=>b.onclick=()=>setTool(b.dataset.tool));
  $('width').oninput=()=>$('widthOut').textContent=`${$('width').value} px`;
  function selectedPath(){const l=doc.layers.find(l=>l.paths.some(p=>p.id===selected));return l&&!l.locked?l.paths.find(p=>p.id===selected):null;}
  for(const id of ['stroke','width','fill'])$(id).addEventListener('change',()=>{const p=selectedPath();if(!p)return;remember();if(id==='stroke')p.stroke=$('stroke').value;if(id==='width')p.stroke_width=Number($('width').value);if(id==='fill')p.fill=$('fill').value;render();});
  $('applyPath').onclick=()=>{const p=selectedPath();if(!p){notify('Select an unlocked path first',true);return;}remember();p.d=$('pathData').value;p.closed=/[zZ]\s*$/.test(p.d);render();};
  $('addLayer').onclick=()=>{const layer={id:uid('layer'),name:`Layer ${doc.layers.length+1}`,visible:true,locked:false,paths:[]};doc.layers.push(layer);layerId=layer.id;render()};
  function detachShared(){sharedBase=null;$('saveBtn').textContent='Save .pen';$('reloadBtn').hidden=true;undo.length=0;redo.length=0;}
  $('newBtn').onclick=()=>{detachShared();doc=fresh();layerId=doc.layers[0].id;draft=[];selected=null;render();notify('New document')};
  async function save(){if(draft.length)finish();if(sharedBase){try{const r=await fetch('/api/document',{method:'PUT',headers:{'content-type':'application/json'},body:JSON.stringify({base:sharedBase,document:doc})});const result=await r.json();if(!r.ok)throw new Error(result.error);sharedBase=structuredClone(doc);notify('Shared document saved');}catch(e){notify(e.message,true)}return;}download(JSON.stringify(doc,null,2),`${doc.name||'untitled'}.pen`,'application/json');notify('.pen file saved')}
  $('reloadBtn').onclick=loadShared;
  document.addEventListener('keydown',e=>{if(/input|select|textarea/i.test(e.target.tagName))return;if((e.ctrlKey||e.metaKey)&&e.key.toLowerCase()==='z'){e.preventDefault();restore(e.shiftKey?redo:undo,e.shiftKey?undo:redo);}},true);
  svg.addEventListener('pointermove',e=>{if(tool==='pen'&&!dragging&&draft.length){hover=e.shiftKey?constrain(point(e),draft[draft.length-1]):point(e);render();}});
  svg.addEventListener('pointerleave',()=>{hover=null;if(!dragging)render();});
  svg.addEventListener('pointerdown',e=>{
    if(tool!=='select'||geometryBusy||e.button!==0)return;
    const kind=e.target.dataset.geometry;
    if(kind || e.target.dataset.id===selected){
      const p=selectedPath();if(!p)return;
      geometryDrag={start:point(e),id:selected,kind:kind||'translate',index:Number(e.target.dataset.index),element:Number(e.target.dataset.element),handle:Number(e.target.dataset.handle),pointerId:e.pointerId};
      svg.setPointerCapture(e.pointerId);e.stopImmediatePropagation();e.preventDefault();
    }
  },true);
  svg.addEventListener('pointerup',async e=>{
    if(!geometryDrag)return;const drag=geometryDrag;geometryDrag=null;svg.releasePointerCapture(e.pointerId);e.stopImmediatePropagation();const p=point(e);
    if(Math.hypot(p.x-drag.start.x,p.y-drag.start.y)<0.1)return;
    const operation=drag.kind==='anchor'?{type:'move-anchor',index:drag.index,x:p.x,y:p.y}:drag.kind==='handle'?{type:'set-handle',element:drag.element,handle:drag.handle,x:p.x,y:p.y}:{type:'translate',dx:p.x-drag.start.x,dy:p.y-drag.start.y};
    await geometryOperation(operation,drag.kind==='translate'&&$('geometryTarget').value==='layer'?null:drag.id);
  },true);
  svg.addEventListener('pointercancel',()=>{geometryDrag=null;render();});
  svg.addEventListener('pointermove',e=>{
    if(!geometryDrag||geometryDrag.kind!=='translate')return;
    const p=point(e),dx=p.x-geometryDrag.start.x,dy=p.y-geometryDrag.start.y;
    const layer=$('geometryTarget').value==='layer'?doc.layers.find(l=>l.paths.some(p=>p.id===geometryDrag.id)):null;
    for(const el of svg.querySelectorAll('path[data-id]'))if(layer?layer.paths.some(p=>p.id===el.dataset.id):el.dataset.id===geometryDrag.id)el.setAttribute('transform',`translate(${dx} ${dy})`);
  });
  $('saveBtn').onclick=save;$('exportBtn').onclick=exportPng;
  $('openFile').onchange=async e=>{try{const parsed=JSON.parse(await e.target.files[0].text());if(parsed.format!=='pentool'||parsed.version!==1||!Array.isArray(parsed.layers)||!parsed.layers.length)throw new Error('Unsupported file');detachShared();doc=parsed;layerId=doc.layers[0]?.id;draft=[];selected=null;render();notify('.pen file opened')}catch(err){notify(`Could not open file: ${err.message}`,true)}finally{e.target.value=''}};
  addEventListener('resize',render); doc=fresh();layerId=doc.layers[0].id;render();
  loadShared();
})();
