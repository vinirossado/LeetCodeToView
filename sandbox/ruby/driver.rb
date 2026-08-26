# Driver de runtime Ruby (TracePoint API) — o equivalente Ruby de
# jdi/Debugger.java (JDI) e com.rs (ICorDebug direto): EXECUTA de verdade o
# código do usuário (ao contrário de static-analyzer/src/ruby_adapter.rs, que
# só faz parsing estático e nunca roda nada) e emite 1 linha JSON por evento
# de step no stdout, no MESMO schema de sandbox/src/events.rs::Event usado
# por Java/C# (line/locals/stack/time_ns/memory_bytes, mais os eventos
# terminais error/timeout/memory_limit_exceeded/stack_overflow/
# step_limit_exceeded/output_truncated).
#
# Diferença estrutural das outras duas linguagens, não um detalhe de
# implementação: TracePoint instrumenta o MESMO processo que roda o código
# do usuário (introspecção in-process) — não existe processo debugger
# separado falando um protocolo remoto (JDWP pro Java, ICorDebug pro C#).
# Este arquivo É o driver E o "alvo" ao mesmo tempo: ele mesmo faz `load` do
# script do usuário, dentro do bloco de tracing.
#
# Normalização de granularidade de step (item aberto do tasks.md, "Runtime
# instrumentado (TracePoint)"): TracePoint(:line) dispara pra CADA linha
# Ruby executada em QUALQUER arquivo (stdlib, gems, o próprio driver) — o
# equivalente cru do JDI seria StepRequest sem nenhum addClassExclusionFilter.
# O filtro `tp.path == SCRIPT_PATH` abaixo é o que corresponde exatamente aos
# `addClassExclusionFilter("java.*"/"jdk.*"/"sun.*")` do Debugger.java: em vez
# de excluir por glob de nome de classe (não existe em Ruby, que não tem
# classes obrigatórias nem convenção de pacote pra glob), filtra por
# caminho de arquivo -- na prática mais preciso, já que cobre TANTO stdlib
# em Ruby puro (ex.: set.rb) QUANTO qualquer gem, sem precisar enumerar
# prefixos. Combinado com só ouvir :call/:return pra pilha (não :b_call/
# :b_return de blocos, nem :c_call/:c_return de métodos em C -- ver mais
# abaixo), isso reproduz a mesma granularidade "STEP_INTO com exclusão" que
# JDI/ICorDebug já estabeleceram: 1 evento por linha de código do USUÁRIO
# executada, empilhando/desempilhando só em chamadas de método reais do
# usuário -- exatamente o que essa 2ª tarefa checklist pede, sem expor a
# diferença crua (TracePoint global, sem esse filtro, seria inutilizável --
# um `puts` sozinho já dispara dezenas de eventos de código interno do
# próprio Kernel#puts/IO).

require 'objspace'
require 'json'

STDOUT.sync = true

MAX_DEPTH = 3
MAX_ARRAY_ELEMENTS = 20
MAX_FIELDS = 20
MAX_STACK_FRAMES = 50
# Cap on how many call-stack frames also get a full `locals` snapshot in the
# `frames` array (per-frame click-to-inspect in the call-stack panel,
# tasks.md's Python-Tutor-inspired recursion-clarity item) -- same value,
# same reasoning as jdi/Debugger.java's MAX_FRAMES_WITH_LOCALS and
# sandbox/src/com/callback/stepping.rs's Rust constant of the same name:
# frame *names* (`stack`) stay cheap even at deep recursion (MAX_STACK_FRAMES
# above already caps that separately), but walking every live frame's own
# Binding#local_variables on EVERY step event is real per-frame cost this
# driver would otherwise pay for frames far beyond what a user could
# plausibly click through anyway.
MAX_FRAMES_WITH_LOCALS = 20
STEP_EVENT_CAP = 5000

script_arg = ARGV[0]
if script_arg.nil? || script_arg.empty?
  STDERR.puts 'uso: driver.rb <script.rb>'
  exit 1
end
SCRIPT_PATH = File.expand_path(script_arg)

# Serialização profunda de valor Ruby -> estrutura JSON-nativa (Hash/Array/
# String/Numeric/bool/nil), com cap de profundidade/elementos/campos e
# detecção de ciclo -- espelha serializeValue de Debugger.java campo a
# campo (MAX_DEPTH/MAX_ARRAY_ELEMENTS/MAX_FIELDS, mesmo texto de marcador
# "...(+N elementos)"/"...(+N frames)", mesmo tratamento de ciclo via id do
# objeto). Ao contrário do Java, isso monta uma ESTRUTURA Ruby nativa e
# delega a serialização final pra stdlib 'json' (JSON.generate/#to_json) em
# vez de montar a string JSON à mão -- decisão deliberada: evita de raiz a
# CLASSE inteira do bug já encontrado e corrigido do lado Java nesta mesma
# sessão (tasks.md, "TAB literal dentro de string JSON corrompendo o evento
# inteiro" -- escapeJson só escapava um subconjunto de caracteres de
# controle até ser corrigido). A stdlib 'json' já escapa todo caractere de
# controle corretamente por construção, então essa classe de bug não pode
# recorrer aqui.
def serialize_value(value, depth, visiting)
  case value
  when nil, true, false, Integer, Float
    value
  when String, Symbol
    value.to_s
  when Array
    id = value.object_id
    return "<ciclo, id=#{id}>" if visiting.include?(id)
    return "<Array[#{value.size}], profundidade máxima>" if depth <= 0

    visiting = visiting.dup << id
    cap = [value.size, MAX_ARRAY_ELEMENTS].min
    items = value.first(cap).map { |v| serialize_value(v, depth - 1, visiting) }
    items << "...(+#{value.size - cap} elementos)" if value.size > cap
    items
  when Hash
    id = value.object_id
    return "<ciclo, id=#{id}>" if visiting.include?(id)
    return "<Hash[#{value.size}], profundidade máxima>" if depth <= 0

    visiting = visiting.dup << id
    result = {}
    count = 0
    value.each do |k, v|
      if count >= MAX_FIELDS
        result['...'] = 'campos omitidos'
        break
      end
      result[k.to_s] = serialize_value(v, depth - 1, visiting)
      count += 1
    end
    result
  else
    # Objetos genéricos: mesma ideia do fallback de campos do Java
    # (obj.referenceType().allFields()), mas via instance_variables — não há
    # equivalente Ruby de "campo declarado" separado de ivar de instância.
    # Sem ivars (ex.: Range, Proc, um Struct sem @campos assinalados),
    # cai pro mesmo fallback textual que o Java usa pra Value não
    # reconhecido (String.valueOf(val)) — aqui, #inspect.
    id = value.object_id
    return "<ciclo, id=#{id}>" if visiting.include?(id)
    return "<#{value.class}, id=#{id}, profundidade máxima>" if depth <= 0

    ivars = value.instance_variables
    if ivars.empty?
      value.inspect
    else
      visiting = visiting.dup << id
      result = {}
      count = 0
      ivars.each do |iv|
        if count >= MAX_FIELDS
          result['...'] = 'campos omitidos'
          break
        end
        begin
          result[iv.to_s] = serialize_value(value.instance_variable_get(iv), depth - 1, visiting)
        rescue StandardError
          result[iv.to_s] = nil
        end
        count += 1
      end
      result
    end
  end
rescue StandardError
  # Fail-open, mesma tolerância do Java: um valor que não consegue ser
  # serializado (ex.: objeto com #inspect quebrado) não derruba o passo
  # inteiro, só aparece como null.
  nil
end

# Pilha de chamadas: mantida manualmente via TracePoint(:call)/(:return) em
# vez de reconstruída a partir de caller_locations dentro do handler de
# :line -- mais simples e mais confiável (não depende de quantos frames
# internos do próprio driver apareceriam no meio). SÓ :call/:return de
# método real (`def`) empilha/desempilha -- blocos (:b_call/:b_return, ex.:
# `arr.each { |x| ... }`) e chamadas de método em C (:c_call/:c_return, ex.:
# o próprio Array#each) deliberadamente NÃO alteram a pilha, mesma
# simplificação que Java já faz (um for-each Java também não ganha frame
# próprio) -- documentado aqui como decisão consciente, não descoberta por
# acidente.
#
# Frame-base "<main>" (não "main" como Java): rótulo real que o próprio
# Ruby já usa em backtraces de nível de topo (`prog.rb:3:in '<main>'`), não
# inventado por este driver.
call_stack = ['<main>']
# Parallel array of Binding objects, ONE PER FRAME, pushed/popped in exact
# lockstep with call_stack above (same :call/:return conditions, same
# index order) -- this is what makes the NEW `frames` array below possible
# (per-frame click-to-inspect locals, tasks.md's Python-Tutor-inspired
# recursion-clarity item, already shipped for Java/C#).
#
# Verified empirically before writing this, not assumed (see tasks.md's own
# note on this item, and this session's throwaway binding_test*.rb scripts):
# a Ruby Binding is a LIVE reference into that call's actual local-variable
# storage, not a value snapshot taken at the moment `binding`/`tp.binding`
# was called. Two things confirmed directly:
#   1. A binding captured once and read again LATER, after the underlying
#      local has been reassigned, returns the CURRENT value, not the value
#      at capture time -- so caching `tp.binding` at :call time (before the
#      method body has even run) and reading `local_variable_get` from it
#      much later (once execution is paused several frames deeper) still
#      returns that FRAME's genuinely current value at the moment of the
#      read, not a stale one.
#   2. `local_variable_defined?`/`local_variable_get` on a binding captured
#      at :call time (i.e. BEFORE any of the method body's own local
#      assignments have executed) still see locals the method assigns
#      LATER in its body -- Ruby pre-allocates local-variable slots for a
#      whole lexical scope at parse time, initialized to nil, so this is
#      not a race, it always works.
#   3. Two concurrent (nested/recursive) invocations of the SAME method get
#      genuinely INDEPENDENT Binding objects/local-variable storage -- a
#      real 5-deep recursive `factorial`-shaped test, read back after the
#      whole call chain unwound, showed each cached frame binding's own
#      local with its OWN correct, distinct value (5,4,3,2,1), not the same
#      value repeated or the innermost/final call's value bleeding into
#      every frame.
# Together these rule out the one failure mode that would have made this
# feature WORSE than not shipping it at all (a per-frame locals view that
# LOOKS real but silently shows stale/collapsed-to-innermost data) --
# see this file's own emit_step/frame_locals below for how it's used.
#
# frame_bindings[0] ('<main>''s own binding) has no :call event to hang off
# of (the top-level script executed via `load` below never fires :call for
# itself) -- filled in lazily by the :line handler instead, the first time
# (and every time, harmless -- see that handler) execution is observed back
# at call_stack.size == 1.
frame_bindings = [nil]
emitted_count = 0
capped = false
t0 = Process.clock_gettime(Process::CLOCK_MONOTONIC, :nanosecond)

def build_stack(call_stack)
  reversed = call_stack.reverse
  frames = reversed.first(MAX_STACK_FRAMES)
  frames += ["...(+#{reversed.size - MAX_STACK_FRAMES} frames)"] if reversed.size > MAX_STACK_FRAMES
  frames
end

# ObjectSpace.memsize_of_all soma o memsize de todo objeto Ruby vivo
# alcançável pelo GC -- não há equivalente direto de
# Runtime.totalMemory()-freeMemory() do JDI aqui (não existe "processo
# alvo" separado pra invocar via protocolo remoto, é o MESMO processo), mas
# essa é a métrica mais honesta disponível in-process: memória de heap
# Ruby genuinamente em uso, não uma estimativa por contagem de slots
# (GC.stat[:heap_live_slots] conta objetos, não bytes -- um Array de 1000
# elementos e um Integer pequeno contam "1" cada, o que não seria
# comparável a bytes de verdade). Limitação honesta, documentada aqui e no
# tasks.md: isso mede heap de objetos Ruby geridos pelo GC, não RSS do
# processo inteiro (não cobre alocação nativa de C extensions fora do GC
# Ruby) -- mesma categoria de "melhor esforço, não bytes exatos de
# processo" que memory_bytes já era pro Java (heap gerenciado, não RSS
# total da JVM).
def read_used_memory
  ObjectSpace.memsize_of_all
rescue StandardError
  nil
end

# Serializa TODAS as local_variables visíveis num Binding -- mesma lógica
# usada tanto pro `locals` de nível superior (frame ativo/innermost) quanto
# pra cada entrada do novo `frames` abaixo (`frame_bindings`'s doc comment
# tem a validação empírica de por que ler um Binding armazenado mais tarde
# é seguro, não obsoleto).
def frame_locals(b)
  locals = {}
  b.local_variables.each do |name|
    locals[name.to_s] =
      begin
        serialize_value(b.local_variable_get(name), MAX_DEPTH, [])
      rescue StandardError
        nil
      end
  end
  locals
end

def emit_step(tp, call_stack, frame_bindings, t0)
  locals = frame_locals(tp.binding)

  # Innermost-first, same order/index as `stack` below (both come from
  # reversing the same manually-tracked, lockstep-maintained arrays) --
  # capped at MAX_FRAMES_WITH_LOCALS, tighter than MAX_STACK_FRAMES (see
  # that constant's doc comment for why). Frame 0 reuses `locals` above
  # (already computed from tp.binding, the live innermost frame) instead of
  # re-walking it -- same efficiency move as jdi/Debugger.java's
  # frameLocalsJson call for i==0 and stepping.rs's walk_call_stack.
  reversed_names = call_stack.reverse
  reversed_bindings = frame_bindings.reverse
  frame_count = [reversed_names.size, MAX_FRAMES_WITH_LOCALS].min
  frames = (0...frame_count).map do |i|
    b = reversed_bindings[i]
    frame_locals_hash = i.zero? ? locals : (b ? frame_locals(b) : {})
    { name: reversed_names[i], locals: frame_locals_hash }
  end

  event = {
    type: 'step',
    line: tp.lineno,
    locals: locals,
    stack: build_stack(call_stack),
    frames: frames,
    time_ns: Process.clock_gettime(Process::CLOCK_MONOTONIC, :nanosecond) - t0,
    memory_bytes: read_used_memory,
  }
  puts event.to_json
rescue StandardError => e
  STDERR.puts({ type: 'error', message: "falha ao emitir evento de step: #{e}" }.to_json)
end

# Declaração antecipada das 3 TracePoint locals: os blocos abaixo referenciam
# `line_tp`/`call_tp`/`return_tp` uns dos outros (pra se desabilitarem
# mutuamente ao atingir o cap) -- em Ruby, uma variável local só é
# reconhecida como tal pelo parser se já apareceu no lado esquerdo de uma
# atribuição ANTES do bloco que a referencia lexicamente. Como os blocos só
# rodam de fato depois que os 3 já foram atribuídos (TracePoint.new não
# habilita nada sozinho), isso é seguro -- mesmo padrão de "declarar nil
# antes, atribuir depois" já necessário em casos assim.
line_tp = nil
call_tp = nil
return_tp = nil

call_tp = TracePoint.new(:call) do |tp|
  if !capped && tp.path == SCRIPT_PATH
    call_stack.push(tp.method_id.to_s)
    frame_bindings.push(tp.binding)
  end
end

return_tp = TracePoint.new(:return) do |tp|
  if !capped && tp.path == SCRIPT_PATH && call_stack.size > 1
    call_stack.pop
    frame_bindings.pop
  end
end

line_tp = TracePoint.new(:line) do |tp|
  next if capped
  next unless tp.path == SCRIPT_PATH

  # '<main>' has no :call event of its own to capture a Binding from (see
  # frame_bindings' doc comment above) -- kept fresh here instead, cheap
  # (a live Binding is just a reference) and harmless to repeat every time
  # we're observed back at the top level.
  frame_bindings[0] = tp.binding if call_stack.size == 1

  emit_step(tp, call_stack, frame_bindings, t0)
  emitted_count += 1
  if emitted_count >= STEP_EVENT_CAP
    capped = true
    line_tp.disable
    call_tp.disable
    return_tp.disable
    puts '{"type":"step_limit_exceeded"}'
  end
end

# target_thread: Thread.current -- restringe a instrumentação à thread
# principal. Decisão de escopo DIFERENTE (mais estreita) da que Java/C# já
# tomaram: aqueles dois DETECTAM ativamente uma 2ª thread real do usuário e
# bloqueiam a execução inteira com um evento de erro dedicado
# ("multi-thread execution is not supported yet"). Este driver NÃO
# implementa essa detecção -- uma 2ª thread real (Thread.new) simplesmente
# roda sem instrumentação nenhuma (nem step, nem pilha), sem travar nem
# corromper o stream de eventos da thread principal, mas também sem o aviso
# explícito que Java/C# dão. Documentado honestamente no tasks.md como
# limitação de escopo, não escondido -- replicar a mesma detecção ativa
# exigiria observar Thread.list crescendo (equivalente ao ThreadStartEvent
# do JDI) e decidir se um novo Thread é "do usuário" ou housekeeping da
# própria stdlib/VM, sem um sinal tão limpo quanto o threadGroup do JVM.
call_tp.enable(target_thread: Thread.current)
return_tp.enable(target_thread: Thread.current)
line_tp.enable(target_thread: Thread.current)

exit_code = 0
begin
  load SCRIPT_PATH
rescue SystemStackError
  # SystemStackError ("stack level too deep") é a classe real do Ruby pra
  # overflow de pilha -- NÃO é StandardError (é Exception direto), por isso
  # precisa de rescue próprio antes do genérico abaixo. MRI reserva uma
  # margem de pilha extra especificamente pra permitir que o corpo de um
  # `rescue SystemStackError` rode mesmo depois do estouro -- validado
  # empiricamente (ver tasks.md) que isso realmente funciona neste driver,
  # não assumido só por já ser um comportamento documentado do MRI em geral.
  puts '{"type":"stack_overflow"}'
  exit_code = 1
rescue SystemExit => e
  # exit/exit! explícito do próprio código do usuário -- não é uma falha do
  # sandbox nem do programa, é o programa decidindo terminar. Repassa o
  # status realmente pedido em vez de tratar como erro genérico.
  exit_code = e.status || 0
rescue Exception => e # rubocop:disable Lint/RescueException
  # Deliberadamente `Exception`, não `StandardError` -- cobre também
  # NoMemoryError/ScriptError (SyntaxError/LoadError inclusos: como não há
  # etapa de compilação separada pro Ruby — ao contrário de javac/dotnet
  # build, que rodam ANTES do driver —, um erro de sintaxe só é descoberto
  # aqui, no `load`, e vira um evento de erro limpo com a mensagem real do
  # parser em vez de uma falha opaca. Ver tasks.md pra essa decisão de
  # design documentada por extenso). Mesmo texto de mensagem que um
  # backtrace Ruby não tratado mostraria no console (classe: mensagem +
  # "\tfrom ..." por linha) -- é a saída do PRÓPRIO programa do usuário,
  # não detalhe interno do sandbox, então mostrar verbatim é seguro e útil
  # (mesmo raciocínio do commit Java equivalente, tasks.md "qualquer
  # exceção não capturada").
  backtrace = (e.backtrace || []).map { |line| "\tfrom #{line}" }.join("\n")
  message = "#{e.class}: #{e.message}"
  message += "\n#{backtrace}" unless backtrace.empty?
  puts({ type: 'error', message: message }.to_json)
  exit_code = 1
ensure
  call_tp.disable
  return_tp.disable
  line_tp.disable
end

exit! exit_code
