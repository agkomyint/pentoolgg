(() => {
  const $ = (id) => document.getElementById(id);
  const svg = $('canvas');
  let doc, pageId, layerId, tool = 'pen', draft = [], selected = null, dragging = null, toastTimer, hover = null;
  const undo = [], redo = [];
  let sharedBase = null, sharedRevision = null;
  let geometryBusy=false,jobController=null;
  $('cancelJob').onclick=()=>jobController?.abort();
  let editGeometry=null, geometryDrag=null;
  const allObjects=l=>[...(l.paths||[]),...(l.texts||[])];
  const activePage=()=>doc.version===3?(doc.pages.find(p=>p.id===pageId)||doc.pages[0]):{id:'page-1',name:'Page 1',canvas:doc.canvas,layers:doc.layers};
  const pageLayers=()=>activePage().layers;
  const pageCanvas=()=>activePage().canvas;
  const objectLayer=(id=selected)=>pageLayers().find(l=>l.id===layerId&&allObjects(l).some(o=>o.id===id));
  function selectedText(){const l=objectLayer();return l&&!l.locked?(l.texts||[]).find(t=>t.id===selected):null;}
  function selectedObject(){return selectedPath()||selectedText();}
  function syncInspector(){const p=selectedPath(),t=selectedText();$('pathData').value=p?.d||'';if(p){if(/^#[0-9a-f]{6}$/i.test(p.stroke))$('stroke').value=p.stroke;$('width').value=p.stroke_width;$('widthOut').textContent=`${p.stroke_width} px`;$('strokeCap').value=p.stroke_linecap||'round';$('strokeJoin').value=p.stroke_linejoin||'round';$('miterLimit').value=p.stroke_miterlimit??4;}if(t){$('textContent').value=t.content;$('textFont').value=t.font_family;$('textSize').value=t.font_size;$('textWeight').value=String(t.font_weight);$('textItalic').checked=!!t.italic;if(/^#[0-9a-f]{6}$/i.test(t.fill))$('textFill').value=t.fill;$('textAlign').value=t.align||'left';$('textSpacing').value=t.letter_spacing||0;$('textLeading').value=t.line_height||1.2;}}
  function textFields(){return{content:$('textContent').value,font:$('textFont').value,size:Number($('textSize').value),weight:Number($('textWeight').value),italic:$('textItalic').checked,fill:$('textFill').value,align:$('textAlign').value,letter_spacing:Number($('textSpacing').value),line_height:Number($('textLeading').value)};}
  async function editText(action){if(geometryBusy)return;const base=JSON.stringify(doc);geometryBusy=true;try{const r=await fetch('/api/text',{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({document:doc,page:pageId,action})});const data=await r.json();if(!r.ok)throw new Error(data.error);if(JSON.stringify(doc)!==base)throw new Error('Artwork changed while calculating. Try again.');remember();doc=data.document;layerId=action.layer;selected=action.type==='remove'?null:action.id;editGeometry=null;syncInspector();render();return true;}catch(e){notify(e.message,true);return false;}finally{geometryBusy=false;}}
  async function editObjects(actions,selectAfter){if(geometryBusy)return false;const base=JSON.stringify(doc);geometryBusy=true;try{const r=await fetch('/api/edit',{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({document:doc,page:pageId,actions})});const data=await r.json();if(!r.ok)throw new Error(data.error);if(JSON.stringify(doc)!==base)throw new Error('Artwork changed while editing. Try again.');remember();doc=data.document;if(selectAfter!==undefined)selected=selectAfter;editGeometry=null;syncInspector();render();notify(`${actions.length} object edit${actions.length===1?'':'s'} applied`);return true;}catch(e){notify(e.message,true);return false;}finally{geometryBusy=false;}}
  $('applyText').onclick=()=>{if(!selectedText()){notify('Select an unlocked text object first',true);return;}editText({type:'set',id:selected,layer:layerId,...textFields()});};
  let loadedFontFaces=[];
  async function loadEmbeddedFonts(){for(const face of loadedFontFaces)document.fonts.delete(face);loadedFontFaces=[];for(const asset of doc.fonts||[]){try{const db=await fetch('/api/font-info',{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({id:asset.id,data:asset.data})});const info=await db.json();if(!db.ok)throw new Error(info.error);const bytes=Uint8Array.from(atob(asset.data),c=>c.charCodeAt(0));for(const f of info.faces){const face=new FontFace(f.family,bytes.buffer,{weight:String(f.weight),style:f.italic?'italic':'normal'});await face.load();document.fonts.add(face);loadedFontFaces.push(face);addFontFamily(f.family);}}catch(e){notify(`Font load failed: ${e.message}`,true);}}render();}
  function addFontFamily(family){if(!family||[...$('fontFamilies').options].some(o=>o.value===family))return;const option=document.createElement('option');option.value=family;$('fontFamilies').append(option);}
  fetch('/api/fonts').then(r=>r.json()).then(info=>info.faces.forEach(f=>addFontFamily(f.family))).catch(()=>{});
  $('importFont').onchange=async e=>{const file=e.target.files[0];if(!file)return;if(geometryBusy)return;const base=JSON.stringify(doc);geometryBusy=true;try{const bytes=new Uint8Array(await file.arrayBuffer());if(bytes.length>6*1024*1024)throw new Error('Font file is too large');let binary='';for(let i=0;i<bytes.length;i+=8192)binary+=String.fromCharCode(...bytes.subarray(i,i+8192));const r=await fetch('/api/font',{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({document:doc,id:uid('font'),data:btoa(binary)})});const info=await r.json();if(!r.ok)throw new Error(info.error);if(JSON.stringify(doc)!==base)throw new Error('Artwork changed while importing. Try again.');remember();doc=info.document;info.fonts.faces.forEach(f=>addFontFamily(f.family));await loadEmbeddedFonts();notify('Font embedded in document');}catch(err){notify(err.message,true);}finally{geometryBusy=false;e.target.value='';}};
  $('importPen').onchange=async e=>{const file=e.target.files[0];if(!file||geometryBusy)return;const base=JSON.stringify(doc);geometryBusy=true;jobController=new AbortController();$('cancelJob').hidden=false;try{if(file.size>64*1024*1024)throw new Error('Import is limited to 64 MiB');const source=JSON.parse(await file.text()),prefix=$('importPrefix').value.trim();const options={destination_page:pageId,source_page:null,prefix:prefix||null,x:Number($('importX').value),y:Number($('importY').value),scale:Number($('importScale').value),rotation:Number($('importRotate').value),expand_canvas:$('importExpand').checked};const r=await fetch('/api/import',{method:'POST',headers:{'content-type':'application/json'},signal:jobController.signal,body:JSON.stringify({document:doc,source,options})});const result=await r.json();if(!r.ok)throw new Error(result.error);if(JSON.stringify(doc)!==base)throw new Error('Artwork changed while importing. Try again.');remember();doc=result.document;layerId=pageLayers().at(-1)?.id;selected=null;const report=$('importResult');report.hidden=false;report.textContent=JSON.stringify(result.summary,null,2);render();await loadEmbeddedFonts();notify(`${Object.keys(result.summary.layers).length} layer(s) imported`);}catch(err){notify(err.name==='AbortError'?'Import cancelled':`Import failed: ${err.message}`,err.name!=='AbortError')}finally{geometryBusy=false;jobController=null;$('cancelJob').hidden=true;e.target.value=''}};
  async function geometryOperation(operation,id=selected){
    if(geometryBusy)return null;
    const layer=id?objectLayer(id):activeLayer();
    if(!layer || (id===null && $('geometryTarget').value!=='layer')){notify('Select a path first',true);return null;}
    const baseDocument=JSON.stringify(doc);
    geometryBusy=true;
    try{const r=await fetch('/api/geometry',{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({document:doc,page:pageId,layer:layer.id,id,operation})});const data=await r.json();if(!r.ok)throw new Error(data.error);if(JSON.stringify(doc)!==baseDocument)throw new Error('Artwork changed while calculating. Try the operation again.');if(!['bounds','nodes','hit'].includes(operation.type)){remember();doc=data.document;editGeometry=null;$('nodeEditor').replaceChildren();render();const p=selectedPath();if(p)$('pathData').value=p.d;}return data.result;}catch(e){notify(e.message,true);return null;}finally{geometryBusy=false;}
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
  async function loadShared(){try{const r=await fetch('/api/document');if(!r.ok)return;const data=await r.json();sharedBase=data.document;sharedRevision=data.revision;doc=structuredClone(sharedBase);pageId=doc.version===3?doc.pages[0]?.id:'page-1';layerId=pageLayers()[0]?.id;draft=[];selected=null;editGeometry=null;undo.length=0;redo.length=0;render();await loadEmbeddedFonts();$('saveBtn').textContent='Save shared';$('reloadBtn').hidden=false;notify('Shared document loaded');}catch(e){notify(`Load failed: ${e.message}`,true)}}
  const snapshot = () => JSON.stringify({doc,pageId,layerId,draft,selected});
  function remember(){undo.push(snapshot());redo.length=0;}
  function restore(from,to){if(!from.length)return;to.push(snapshot());({doc,pageId,layerId,draft,selected}=JSON.parse(from.pop()));dragging=null;hover=null;editGeometry=null;$('nodeEditor').replaceChildren();syncInspector();render();loadEmbeddedFonts();}
  const uid = (prefix) => `${prefix}-${crypto.randomUUID ? crypto.randomUUID() : Date.now().toString(36)}`;
  const fresh = () => {const layer={id:uid('layer'),name:'Layer 1',visible:true,locked:false,paths:[],texts:[]};return{format:'pentool',version:3,name:'Untitled',pages:[{id:'page-1',name:'Page 1',canvas:{width:1200,height:800,background:'#ffffff'},layers:[layer]}],fonts:[]}};
  const activeLayer = () => pageLayers().find(l => l.id === layerId) || pageLayers()[0];
  const ns = 'http://www.w3.org/2000/svg';

  function notify(message, error=false) { const t=$('toast'); t.textContent=message; t.className=`toast show${error?' error':''}`; clearTimeout(toastTimer); toastTimer=setTimeout(()=>t.className='toast',2600); }
  function escapeXml(s) { return String(s).replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&apos;'}[c])); }
  function render() {
    const canvas=pageCanvas(),layers=pageLayers();
    svg.setAttribute('viewBox',`0 0 ${canvas.width} ${canvas.height}`);
    const maxW=Math.max(280,document.querySelector('.stage').clientWidth-80), maxH=Math.max(240,document.querySelector('.stage').clientHeight-80);
    const scale=Math.min(maxW/canvas.width,maxH/canvas.height,1);
    svg.style.width=`${canvas.width*scale}px`; svg.style.height=`${canvas.height*scale}px`; svg.style.background=canvas.background;
    svg.replaceChildren();
    layers.filter(l=>l.visible).forEach(layer=>{(layer.paths||[]).forEach(p=>{
      const el=document.createElementNS(ns,'path');
      for(const [k,v] of Object.entries({d:p.d,stroke:p.stroke,'stroke-width':p.stroke_width,fill:p.fill,'stroke-linecap':p.stroke_linecap||'round','stroke-linejoin':p.stroke_linejoin||'round','stroke-miterlimit':p.stroke_miterlimit??4})) el.setAttribute(k,v);
      el.dataset.id=p.id;el.dataset.layer=layer.id;if(p.id===selected&&layer.id===layerId)el.classList.add('selected');svg.append(el);
    });for(const t of layer.texts||[]){const wrapper=document.createElementNS(ns,'g');wrapper.dataset.id=t.id;wrapper.dataset.layer=layer.id;wrapper.dataset.kind='text';const el=document.createElementNS(ns,'text');for(const [k,v] of Object.entries({'xml:space':'preserve',transform:`matrix(${(t.transform||[1,0,0,1,0,0]).join(' ')})`,'font-family':t.font_family,'font-size':t.font_size,'font-weight':t.font_weight,'font-style':t.italic?'italic':'normal',fill:t.fill,'text-anchor':({left:'start',center:'middle',right:'end'})[t.align||'left'],'letter-spacing':t.letter_spacing||0}))el.setAttribute(k,v);t.content.replace(/\r\n?/g,'\n').split('\n').forEach((line,i)=>{const span=document.createElementNS(ns,'tspan');span.setAttribute('x',t.x);span.setAttribute('y',t.y+i*t.font_size*(t.line_height||1.2));span.textContent=line;el.append(span);});if(t.id===selected&&layer.id===layerId)wrapper.classList.add('selected');wrapper.append(el);svg.append(wrapper);}});
    if(draft.length){const p=document.createElementNS(ns,'path');p.setAttribute('d',pathData(draft));p.setAttribute('stroke',$('stroke').value);p.setAttribute('stroke-width',$('width').value);p.setAttribute('fill','none');p.setAttribute('stroke-dasharray','6 5');p.setAttribute('stroke-linecap',$('strokeCap').value);p.setAttribute('stroke-linejoin',$('strokeJoin').value);p.setAttribute('stroke-miterlimit',$('miterLimit').value);p.setAttribute('pointer-events','none');svg.append(p);}
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
    renderPages();renderLayers();
  }
  const n = value => value.toFixed(2);
  const pathData = points => points.map((p,i)=>{
    if(!i) return `M ${n(p.x)} ${n(p.y)}`;
    const prev=points[i-1], a=prev.out||prev, b=p.in||p;
    return `C ${n(a.x)} ${n(a.y)} ${n(b.x)} ${n(b.y)} ${n(p.x)} ${n(p.y)}`;
  }).join(' ');
  function point(e){const r=svg.getBoundingClientRect(),canvas=pageCanvas();return{x:(e.clientX-r.left)*canvas.width/r.width,y:(e.clientY-r.top)*canvas.height/r.height};}
  function finish(closed=false){if(draft.length>1){remember();const points=closed?[...draft,draft[0]]:draft;activeLayer().paths.push({id:uid('path'),d:pathData(points)+(closed?' Z':''),stroke:$('stroke').value,stroke_width:Number($('width').value),fill:$('fill').value,stroke_linecap:$('strokeCap').value,stroke_linejoin:$('strokeJoin').value,stroke_miterlimit:Number($('miterLimit').value),closed});notify(closed?'Closed path added':'Path added');}draft=[];hover=null;dragging=null;render();}
  function constrain(p,origin){const dx=p.x-origin.x,dy=p.y-origin.y,length=Math.hypot(dx,dy),angle=Math.round(Math.atan2(dy,dx)/(Math.PI/4))*Math.PI/4;return{x:origin.x+Math.cos(angle)*length,y:origin.y+Math.sin(angle)*length};}
  function near(a,b){return Math.hypot(a.x-b.x,a.y-b.y)<10*pageCanvas().width/svg.getBoundingClientRect().width;}
  function renderPages(){
    const root=$('pages');root.replaceChildren();
    const pages=doc.version===3?doc.pages:[{id:'page-1',name:'Page 1'}];
    pages.forEach((page,index)=>{const row=document.createElement('div');row.className=`page-row${page.id===pageId?' active':''}`;
      const name=document.createElement('button');name.className='page-name';name.textContent=page.name;name.onclick=()=>{pageId=page.id;layerId=activePage().layers[0]?.id;selected=null;draft=[];editGeometry=null;render()};
      const rename=document.createElement('button');rename.textContent='✎';rename.title='Rename page';rename.onclick=()=>{const value=prompt('Page name',page.name);if(value){remember();page.name=value;render()}};
      const duplicate=document.createElement('button');duplicate.textContent='⧉';duplicate.title='Duplicate page';duplicate.onclick=()=>{remember();if(doc.version!==3)return;const copy=structuredClone(page);copy.id=uid('page');copy.name=`${page.name} copy`;doc.pages.splice(index+1,0,copy);pageId=copy.id;layerId=copy.layers[0]?.id;selected=null;render()};
      const up=document.createElement('button');up.textContent='↑';up.title='Move page up';up.disabled=index===0;up.onclick=()=>{remember();doc.pages.splice(index-1,0,doc.pages.splice(index,1)[0]);render()};
      const down=document.createElement('button');down.textContent='↓';down.title='Move page down';down.disabled=index===pages.length-1;down.onclick=()=>{remember();doc.pages.splice(index+1,0,doc.pages.splice(index,1)[0]);render()};
      const remove=document.createElement('button');remove.textContent='×';remove.title='Delete page';remove.disabled=pages.length===1;remove.onclick=()=>{if(pages.length===1)return;remember();const i=doc.pages.findIndex(p=>p.id===page.id);doc.pages.splice(i,1);if(pageId===page.id)pageId=doc.pages[Math.min(i,doc.pages.length-1)].id;layerId=activePage().layers[0]?.id;selected=null;render()};
      row.append(name,rename,duplicate,up,down,remove);root.append(row);
    });
  }
  function renderLayers(){
    const root=$('layers'),query=$('layerSearch').value.trim().toLowerCase();root.replaceChildren();
    let renderedObjects=0,totalObjects=0;
    [...pageLayers()].reverse().forEach(layer=>{
      const matching=allObjects(layer).filter(o=>!query||layer.id.toLowerCase().includes(query)||layer.name.toLowerCase().includes(query)||o.id.toLowerCase().includes(query)||(o.content||'').toLowerCase().includes(query));totalObjects+=matching.length;const objects=matching.slice(0,Math.max(0,500-renderedObjects));renderedObjects+=objects.length;
      if(query&&!objects.length&&!layer.id.toLowerCase().includes(query)&&!layer.name.toLowerCase().includes(query))return;
      const row=document.createElement('div');row.className=`layer${layer.id===layerId?' active':''}`;
      const eye=document.createElement('button');eye.setAttribute('aria-label',`${layer.visible?'Hide':'Show'} ${layer.name}`);eye.textContent=layer.visible?'◉':'○';
      eye.onclick=()=>{remember();layer.visible=!layer.visible;render()};
      const name=document.createElement('button');name.className='layer-name';name.textContent=layer.name;
      name.onclick=()=>{layerId=layer.id;selected=null;editGeometry=null;$('nodeEditor').replaceChildren();syncInspector();render()};
      const lock=document.createElement('button');lock.setAttribute('aria-label',`${layer.locked?'Unlock':'Lock'} ${layer.name}`);lock.textContent=layer.locked?'🔒':'·';
      lock.onclick=()=>{remember();layer.locked=!layer.locked;editGeometry=null;syncInspector();render()};
      row.append(eye,name,lock);
      const list=document.createElement('div');list.className='layer-objects';
      objects.forEach(o=>{
        const kind=(layer.paths||[]).includes(o)?'path':'text',stack=kind==='path'?layer.paths:layer.texts,index=stack.indexOf(o);
        const item=document.createElement('div');item.className=`object-row${selected===o.id&&layerId===layer.id?' active':''}`;
        const choose=document.createElement('button');choose.className='object-name';choose.innerHTML=`<span class="object-kind">${kind}</span>${escapeXml(o.id)}`;choose.title=o.content||o.id;choose.onclick=()=>{layerId=layer.id;selected=o.id;editGeometry=null;syncInspector();render()};
        const rename=document.createElement('button');rename.textContent='✎';rename.title='Rename';rename.onclick=()=>{const next=prompt('New object ID',o.id);if(next&&next!==o.id)editObjects([{type:'rename',id:o.id,layer:layer.id,new_id:next}],next)};
        const duplicate=document.createElement('button');duplicate.textContent='⧉';duplicate.title='Duplicate';duplicate.onclick=()=>{const next=uid(o.id);editObjects([{type:'duplicate',id:o.id,layer:layer.id,new_id:next}],next)};
        const up=document.createElement('button');up.textContent='↑';up.title='Move forward';up.disabled=index>=stack.length-1;up.onclick=()=>editObjects([{type:'reorder',id:o.id,layer:layer.id,index:index+1}],o.id);
        const down=document.createElement('button');down.textContent='↓';down.title='Move backward';down.disabled=index===0;down.onclick=()=>editObjects([{type:'reorder',id:o.id,layer:layer.id,index:index-1}],o.id);
        const remove=document.createElement('button');remove.textContent='×';remove.title='Delete';remove.onclick=()=>editObjects([{type:'remove',id:o.id,layer:layer.id}],null);
        for(const b of [rename,duplicate,up,down,remove])b.disabled=b.disabled||layer.locked;
        item.append(choose,rename,duplicate,up,down,remove);list.append(item);
      });
      row.append(list);root.append(row);
    });
    if(totalObjects>renderedObjects){const note=document.createElement('p');note.className='text-help';note.textContent=`Showing ${renderedObjects} of ${totalObjects} matching objects. Refine search to inspect more.`;root.append(note)}
  }
  function download(data,name,type){const a=document.createElement('a');a.href=URL.createObjectURL(new Blob([data],{type}));a.download=name;a.click();setTimeout(()=>URL.revokeObjectURL(a.href),1000);}
  async function exportPng(){const btn=$('exportBtn');btn.disabled=true;btn.setAttribute('aria-busy','true');jobController=new AbortController();$('cancelJob').hidden=false;try{const r=await fetch(`/api/render/png?page=${encodeURIComponent(pageId)}`,{method:'POST',headers:{'content-type':'application/json'},signal:jobController.signal,body:JSON.stringify(doc)});if(!r.ok){const j=await r.json();throw new Error(j.error)}download(await r.blob(),`${doc.name || 'artwork'}-${pageId}.png`,'image/png');notify('PNG exported');}catch(e){notify(e.name==='AbortError'?'Export cancelled':`Export failed: ${e.message}`,e.name!=='AbortError')}finally{jobController=null;$('cancelJob').hidden=true;btn.disabled=false;btn.removeAttribute('aria-busy')}}
  svg.addEventListener('pointerdown',e=>{if(e.button!==0||geometryBusy)return;if(tool==='text'){const l=activeLayer();if(l.locked||!l.visible){notify('Choose a visible, unlocked layer',true);return;}const p=point(e);editText({type:'put',id:uid('text'),layer:l.id,x:p.x,y:p.y,...textFields()}).then(ok=>{if(ok)setTool('select');});return;}if(tool==='pen'&&!activeLayer().locked&&activeLayer().visible){const p=point(e);if(draft.length>1&&near(p,draft[0])){finish(true);return;}remember();dragging={index:draft.length,start:p};draft.push(e.shiftKey&&draft.length?constrain(p,draft[draft.length-1]):p);hover=null;svg.setPointerCapture(e.pointerId);render()}else if(tool==='select'){const target=e.target.closest('[data-id]'),l=pageLayers().find(l=>l.id===target?.dataset.layer);selected=l&&!l.locked?target.dataset.id:null;if(l)layerId=l.id;editGeometry=null;$('nodeEditor').replaceChildren();syncInspector();render()}});
  svg.addEventListener('pointermove',e=>{if(!dragging)return;const p=point(e), anchor=draft[dragging.index], dx=p.x-dragging.start.x,dy=p.y-dragging.start.y;anchor.out={x:anchor.x+dx,y:anchor.y+dy};anchor.in={x:anchor.x-dx,y:anchor.y-dy};render()});
  svg.addEventListener('pointerup',e=>{if(dragging){dragging=null;svg.releasePointerCapture(e.pointerId)}});
  svg.addEventListener('dblclick',e=>{e.preventDefault();if(tool==='select'&&selectedText()){$('textContent').focus();$('textContent').select();}else if(tool==='pen')finish();});
  document.addEventListener('keydown',e=>{if(/input|select|textarea/i.test(e.target.tagName))return;if((e.key==='Enter')&&draft.length){finish();e.preventDefault()}if(e.key==='Escape'){draft=[];selected=null;editGeometry=null;render()}if((e.key==='Delete'||e.key==='Backspace')&&selectedObject()){if(selectedText())editText({type:'remove',id:selected,layer:layerId});else{remember();activeLayer().paths=activeLayer().paths.filter(p=>p.id!==selected);selected=null;editGeometry=null;render();}e.preventDefault()}if(e.ctrlKey||e.metaKey){if(e.key.toLowerCase()==='s'){e.preventDefault();save();}return;}if(e.key.toLowerCase()==='p')setTool('pen');if(e.key.toLowerCase()==='v')setTool('select');if(e.key.toLowerCase()==='t')setTool('text');});
  function setTool(value){if(draft.length&&value!=='pen')finish();tool=value;document.querySelectorAll('.tool').forEach(b=>{const on=b.dataset.tool===tool;b.classList.toggle('active',on);b.setAttribute('aria-pressed',on)});$('hint').textContent=tool==='pen'?'Click to place points · Enter to finish · Esc to cancel':tool==='text'?'Set text and font in the inspector · Click to place':tool==='select'?'Click an object · Drag to move · Double-click text to edit':'Pan tool is reserved for the next release';}
  document.querySelectorAll('.tool').forEach(b=>b.onclick=()=>setTool(b.dataset.tool));
  $('width').oninput=()=>$('widthOut').textContent=`${$('width').value} px`;
  function selectedPath(){const l=objectLayer();return l&&!l.locked?l.paths.find(p=>p.id===selected):null;}
  for(const id of ['stroke','width','fill'])$(id).addEventListener('change',()=>{const p=selectedPath();if(!p)return;remember();if(id==='stroke')p.stroke=$('stroke').value;if(id==='width')p.stroke_width=Number($('width').value);if(id==='fill')p.fill=$('fill').value;render();});
  $('applyPath').onclick=()=>{const p=selectedPath();if(!p){notify('Select an unlocked path first',true);return;}remember();p.d=$('pathData').value;p.closed=/[zZ]\s*$/.test(p.d);render();};
  for(const id of ['strokeCap','strokeJoin','miterLimit'])$(id).addEventListener('change',()=>{
    const limit=Number($('miterLimit').value);
    if(!Number.isFinite(limit)||limit<1||limit>1000){notify('Miter limit must be 1–1000',true);return;}
    const p=selectedPath();if(p){remember();p.stroke_linecap=$('strokeCap').value;p.stroke_linejoin=$('strokeJoin').value;p.stroke_miterlimit=limit;}render();
  });
  $('addLayer').onclick=()=>{remember();const layers=pageLayers(),layer={id:uid('layer'),name:`Layer ${layers.length+1}`,visible:true,locked:false,paths:[],texts:[]};layers.push(layer);layerId=layer.id;selected=null;render()};
  $('addPage').onclick=()=>{remember();if(doc.version!==3){doc={format:'pentool',version:3,name:doc.name,pages:[{id:'page-1',name:'Page 1',canvas:doc.canvas,layers:doc.layers}],fonts:doc.fonts||[]};pageId='page-1';}const id=uid('page');doc.pages.push({id,name:`Page ${doc.pages.length+1}`,canvas:{width:pageCanvas().width,height:pageCanvas().height,background:pageCanvas().background},layers:[{id:uid('layer'),name:'Layer 1',visible:true,locked:false,paths:[],texts:[]}]});pageId=id;layerId=activePage().layers[0].id;selected=null;draft=[];render()};
  let searchTimer;$('layerSearch').oninput=()=>{clearTimeout(searchTimer);searchTimer=setTimeout(renderLayers,120)};
  function detachShared(){sharedBase=null;sharedRevision=null;$('saveBtn').textContent='Save .pen';$('reloadBtn').hidden=true;undo.length=0;redo.length=0;}
  $('newBtn').onclick=()=>{detachShared();doc=fresh();pageId=doc.pages[0].id;layerId=pageLayers()[0].id;draft=[];selected=null;editGeometry=null;loadEmbeddedFonts();render();notify('New document')};
  async function save(){if(geometryBusy){notify('Wait for the current edit to finish',true);return;}if(draft.length)finish();if(sharedBase){try{const r=await fetch('/api/document',{method:'PUT',headers:{'content-type':'application/json'},body:JSON.stringify({revision:sharedRevision,base:sharedBase,document:doc})});const result=await r.json();if(!r.ok)throw new Error(result.error);sharedBase=structuredClone(doc);sharedRevision=result.revision;notify('Shared document saved');}catch(e){notify(e.message,true)}return;}download(JSON.stringify(doc,null,2),`${doc.name||'untitled'}.pen`,'application/json');notify('.pen file saved')}
  $('reloadBtn').onclick=loadShared;
  document.addEventListener('keydown',e=>{if(/input|select|textarea/i.test(e.target.tagName))return;if((e.ctrlKey||e.metaKey)&&e.key.toLowerCase()==='z'){e.preventDefault();restore(e.shiftKey?redo:undo,e.shiftKey?undo:redo);}},true);
  svg.addEventListener('pointermove',e=>{if(tool==='pen'&&!dragging&&draft.length){hover=e.shiftKey?constrain(point(e),draft[draft.length-1]):point(e);render();}});
  svg.addEventListener('pointerleave',()=>{hover=null;if(!dragging)render();});
  svg.addEventListener('pointerdown',e=>{
    if(tool!=='select'||geometryBusy||e.button!==0)return;
    const kind=e.target.dataset.geometry;
    const target=e.target.closest('[data-id]');
    if(kind || (target?.dataset.id===selected&&target?.dataset.layer===layerId)){
      const p=selectedObject();if(!p)return;
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
    const layer=$('geometryTarget').value==='layer'?objectLayer(geometryDrag.id):null;
    for(const el of svg.querySelectorAll('[data-id]'))if(el.dataset.layer===layerId&&(layer||el.dataset.id===geometryDrag.id))el.setAttribute('transform',`translate(${dx} ${dy})`);
  });
  $('saveBtn').onclick=save;$('exportBtn').onclick=exportPng;
  $('openFile').onchange=async e=>{try{const parsed=JSON.parse(await e.target.files[0].text()),legacy=[1,2].includes(parsed.version)&&Array.isArray(parsed.layers)&&parsed.layers.length,multi=parsed.version===3&&Array.isArray(parsed.pages)&&parsed.pages.length;if(parsed.format!=='pentool'||(!legacy&&!multi))throw new Error('Unsupported file');detachShared();doc=parsed;pageId=multi?doc.pages[0].id:'page-1';layerId=pageLayers()[0]?.id;draft=[];selected=null;editGeometry=null;render();await loadEmbeddedFonts();notify('.pen file opened')}catch(err){notify(`Could not open file: ${err.message}`,true)}finally{e.target.value=''}};
  async function loadAssets(){const root=$('assetResults'),query=$('assetSearch').value.trim();root.innerHTML='<p class="text-help">Searching…</p>';try{const response=await fetch(`/api/assets?query=${encodeURIComponent(query)}&limit=60`),result=await response.json();if(!response.ok)throw new Error(result.error);root.replaceChildren();for(const asset of result.assets){const card=document.createElement('div');card.className='asset-card';const title=document.createElement('strong');title.textContent=asset.name;title.title=asset.id;const meta=document.createElement('small');meta.textContent=`${asset.library} · ${asset.id} · ${asset.version}`;const add=document.createElement('button');add.textContent='Add copy';add.onclick=()=>insertAsset(`${asset.library}/${asset.id}`);card.append(title,meta,add);root.append(card)}if(!result.assets.length)root.innerHTML='<p class="text-help">No matching assets.</p>';}catch(error){root.innerHTML=`<p class="text-help">${escapeXml(error.message)}</p>`;}}
  async function insertAsset(spec){try{const sourceResponse=await fetch(`/api/asset?spec=${encodeURIComponent(spec)}`),source=await sourceResponse.json();if(!sourceResponse.ok)throw new Error(source.error);remember();const prefix=`asset-${Date.now().toString(36)}`,response=await fetch('/api/import',{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({document:doc,source:source.document,options:{destination_page:pageId,source_page:source.document.asset?.entry_page||null,prefix,x:0,y:0,scale:1,rotation:0,expand_canvas:false}})}),result=await response.json();if(!response.ok)throw new Error(result.error);doc=result.document;const ids=Object.values(result.summary.layers);if($('assetMode').value==='instance'){doc.instances??=[];doc.instances.push({id:uid('instance'),library:source.asset.library,asset_id:source.asset.id,asset_version:source.asset.version,content_hash:source.asset.content_hash,layer_ids:ids,page_id:pageId,transform:[1,0,0,1,0,0],visible:true,overrides:{},previous:[]})}layerId=ids.at(-1)||layerId;selected=null;render();notify(`${source.asset.name} added as ${$('assetMode').value}`);}catch(error){notify(`Could not add asset: ${error.message}`,true)}}
  $('exportLayerAsset').onclick=()=>{const id=$('assetExportId').value.trim(),name=$('assetExportName').value.trim();if(!/^[A-Za-z0-9._-]+(?:\/[A-Za-z0-9._-]+)*$/.test(id)||!name){notify('Enter a portable asset ID and name',true);return}const layer=structuredClone(activeLayer()),canvas=structuredClone(pageCanvas()),assetDoc={format:'pentool',version:3,name,pages:[{id:'asset',name,canvas,layers:[layer]}],fonts:structuredClone(doc.fonts||[]),asset:{schema:1,id,name,description:'',asset_version:'0.1.0',kind:'component',author:'',license:'',tags:[],category:'',entry_page:'asset',properties:{}}};download(JSON.stringify(assetDoc,null,2),`${id.split('/').at(-1)}.pen`,'application/json');notify('Active layer exported as a reusable asset')};
  $('searchRegistry').onclick=async()=>{const source=$('registrySource').value.trim(),query=$('assetSearch').value.trim(),root=$('registryResults');if(!source){notify('Enter a registry URL or folder',true);return}root.innerHTML='<p class="text-help">Searching registry…</p>';try{const response=await fetch(`/api/registry/search?source=${encodeURIComponent(source)}&query=${encodeURIComponent(query)}`),data=await response.json();if(!response.ok)throw new Error(data.error);root.replaceChildren();for(const item of data.results){const card=document.createElement('div');card.className='asset-card';const title=document.createElement('strong');title.textContent=item.name;const version=item.versions.at(-1),meta=document.createElement('small');meta.textContent=item.versions.join(', ');const install=document.createElement('button');install.textContent='Install';install.onclick=async()=>{const r=await fetch('/api/registry/install',{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({source,spec:`${item.name}@${version}`})}),result=await r.json();if(!r.ok){notify(result.error,true);return}notify(`${item.name}@${version} installed`);loadAssets()};card.append(title,meta,install);root.append(card)}if(!data.results.length)root.innerHTML='<p class="text-help">No packages found.</p>'}catch(error){root.innerHTML=`<p class="text-help">${escapeXml(error.message)}</p>`}};
  let assetTimer;$('assetSearch').oninput=()=>{clearTimeout(assetTimer);assetTimer=setTimeout(loadAssets,150)};$('refreshAssets').onclick=loadAssets;loadAssets();
  addEventListener('resize',render); doc=fresh();pageId=doc.pages[0].id;layerId=pageLayers()[0].id;render();
  loadShared();
})();
