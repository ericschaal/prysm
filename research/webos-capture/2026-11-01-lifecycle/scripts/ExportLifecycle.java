// Offline diagnostic export; never changes or exports the input executable.
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.DecompInterface;
import ghidra.app.decompiler.DecompileResults;
import ghidra.program.model.listing.Function;
import ghidra.program.model.listing.FunctionIterator;
import java.io.PrintWriter;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Paths;

public class ExportLifecycle extends GhidraScript {
 public void run() throws Exception {
  String[] args=getScriptArgs();
  if(args.length!=1) throw new IllegalArgumentException("output path required");
  DecompInterface decompiler=new DecompInterface();
  if(!decompiler.openProgram(currentProgram)) throw new IllegalStateException(decompiler.getLastMessage());
  int exported=0,failed=0;
  try(PrintWriter out=new PrintWriter(Files.newBufferedWriter(Paths.get(args[0]),StandardCharsets.UTF_8))){
   out.println("// Program: "+currentProgram.getName()+" language="+currentProgram.getLanguageID()+" compiler="+currentProgram.getCompilerSpec().getCompilerSpecID()+" imagebase="+currentProgram.getImageBase());
   FunctionIterator functions=currentProgram.getFunctionManager().getFunctions(true);
   while(functions.hasNext()&&!monitor.isCancelled()){
    Function f=functions.next();String name=f.getName(true);
    if(f.isExternal() || !(name.contains("VtDriver") || name.contains("ResourceManagerClient") || name.contains("LS2Interface") || name.contains("Permission") || name.contains("vtCapture_") || name.contains("Thread::") || name.contains("UMSConnector_impl")))continue;
    out.println("\n// FUNCTION "+f.getEntryPoint()+" "+name);
    DecompileResults result=decompiler.decompileFunction(f,20,monitor);
    if(result.decompileCompleted()){out.println(result.getDecompiledFunction().getC());exported++;}
    else{out.println("// FAILED "+result.getErrorMessage());failed++;}
    out.flush();
   }
   out.println("// EXPORTED="+exported+" FAILED="+failed);
  }finally{decompiler.dispose();}
  if(exported==0)throw new IllegalStateException("No lifecycle functions exported");
  println("Lifecycle export: "+exported+" functions, "+failed+" failures");
 }
}
