/*
 * Copyright 2018 Fluence Labs Limited
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

package fluence.vm
import java.lang.reflect.Method
import java.nio.{ByteBuffer, ByteOrder}

import asmble.compile.jvm.AsmExtKt
import asmble.run.jvm.Module.Compiled
import asmble.run.jvm.ScriptContext
import cats.{Functor, Monad}
import cats.data.EitherT
import cats.effect.{IO, LiftIO}
import fluence.crypto.CryptoError
import fluence.vm.AsmbleWasmVm.FunctionId
import fluence.vm.ModuleInstance.nameAsStr
import fluence.vm.VmError.WasmVmError.{ApplyError, InvokeError}
import fluence.vm.VmError.{InitializationError, InternalVmError, NoSuchFnError, TrapError}

import scala.language.higherKinds
import scala.util.Try

/**
 * Wasm Module instance wrapper.
 *
 * @param name module name (can be empty)
 * @param instance wrapped instance of module
 * @param memory memory of this module
 */
case class ModuleInstance(
  name: String,
  instance: Any,
  private[vm] val memory: Option[ByteBuffer]
) {

  private val allocateFunction: Option[WasmFunction] =
    functionsIndex.get(FunctionId(modules.head.name, AsmExtKt.getJavaIdent(allocateFunctionName)))

  private val deallocateFunction: Option[WasmFunction] =
    functionsIndex.get(FunctionId(modules.head.name, AsmExtKt.getJavaIdent(deallocateFunctionName)))

  private val invokeFunction: Option[WasmFunction] =
    functionsIndex.get(FunctionId(modules.head.name, AsmExtKt.getJavaIdent("invoke")))

  // TODO : In the future, it should be rewritten with cats.effect.resource
  /**
    * Allocates memory in Wasm module of supplied size by allocateFunction.
    *
    * @param size size of memory that need to be allocated
    * @tparam F a monad with an ability to absorb 'IO'
    */
  private def allocate[F[_]: LiftIO: Monad](size: Int): EitherT[F, InvokeError, AnyRef] = {
    allocateFunction match {
      case Some(fn) => fn(size.asInstanceOf[AnyRef] :: Nil)
      case _ =>
        EitherT.leftT(
          NoSuchFnError(s"Unable to find the function for memory allocation with the name=$allocateFunctionName")
        )
    }
  }

  /**
    * Deallocates previously allocated memory in Wasm module by deallocateFunction.
    *
    * @param offset address of memory to deallocate
    * @tparam F a monad with an ability to absorb 'IO'
    */
  private def deallocate[F[_]: LiftIO: Monad](offset: Int, size: Int): EitherT[F, InvokeError, AnyRef] = {
    deallocateFunction match {
      case Some(fn) => fn(offset.asInstanceOf[AnyRef] :: size.asInstanceOf[AnyRef] :: Nil)
      case _ =>
        EitherT.leftT(
          NoSuchFnError(s"Unable to find the function for memory deallocation with the name=$deallocateFunctionName")
        )
    }
  }


  /**
   * Returns hash of all significant inner state of this VM.
   *
   * @param hashFn a hash function
   */
  def innerState[F[_]: Monad](
    hashFn: Array[Byte] ⇒ EitherT[F, CryptoError, Array[Byte]]
  ): EitherT[F, InternalVmError, Array[Byte]] =
    memory match {
      case Some(mem) ⇒
        for {

          memoryAsArray ← EitherT
            .fromEither[F](
              Try {
                // need a shallow ByteBuffer copy to avoid modifying the original one used by Asmble
                val wasmMemoryView = mem.duplicate()
                wasmMemoryView.clear()
                val arr = new Array[Byte](wasmMemoryView.capacity())
                wasmMemoryView.get(arr)
                arr
              }.toEither
            )
            .leftMap { e ⇒
              InternalVmError(
                s"Presenting memory as an array for module=${nameAsStr(name)} failed",
                Some(e)
              )
            }

          vmStateAsHash ← hashFn(memoryAsArray).leftMap { e ⇒
            InternalVmError(
              s"Getting internal state for module=${nameAsStr(name)} failed",
              Some(e)
            )
          }

        } yield vmStateAsHash

      case None ⇒
        // Returning empty array is a temporary solution.
        // It's valid situation when a module doesn't have a memory.
        // When the Stack will be accessible we will return hash of the Stack with registers.
        EitherT.rightT(Array.emptyByteArray)
    }

  override def toString: String = s"Module(${nameAsStr(name)}, memory=$memory)"
}

object ModuleInstance {

  /**
   * Creates instance for specified module.
   *
   * @param moduleDescription a description of the module
   * @param scriptContext a context for the module operation
   */
  def apply(
    moduleDescription: Compiled,
    scriptContext: ScriptContext
  ): Either[ApplyError, ModuleInstance] =
    for {

      // creating module instance
      moduleInstance <- Try(moduleDescription.instance(scriptContext)).toEither.left.map { e ⇒
        // todo method 'instance' must throw both an initialization error and a
        // Trap error, but now they can't be separated
        InitializationError(
          s"Unable to initialize module=${nameAsStr(moduleDescription.getName)}",
          Some(e)
        )
      }

      // getting memory field with reflection from module instance
      memory ← Try {
        // It's ok if a module doesn't have a memory
        val memoryMethod = Try(moduleInstance.getClass.getMethod("getMemory")).toOption
        memoryMethod.map(_.invoke(moduleInstance).asInstanceOf[ByteBuffer])
      }.toEither.left.map { e ⇒
        InternalVmError(
          s"Unable to getting memory from module=${nameAsStr(moduleDescription.getName)}",
          Some(e)
        )
      }

    } yield ModuleInstance(Option(moduleDescription.getName), moduleInstance, memory)

  /**
    * Representation for each Wasm function. Contains reference to module instance
    * and java method [[java.lang.reflect.Method]].
    *
    * @param javaMethod a java method [[java.lang.reflect.Method]] for calling function.
    * @param module the object the underlying method is invoked from.
    *               This is an instance for the current module, it contains
    *               all inner state of the module, like memory.
    */
  case class WasmFunction(
   fnId: FunctionId,
   javaMethod: Method,
   module: ModuleInstance
  ) {

    /**
      * Invokes this function with arguments.
      *
      * @param args arguments for calling this function.
      * @tparam F a monad with an ability to absorb 'IO'
      */
    def apply[F[_]: Functor: LiftIO](args: List[AnyRef]): EitherT[F, InvokeError, AnyRef] =
      EitherT(IO(javaMethod.invoke(module.instance, args: _*)).attempt.to[F])
        .leftMap(e ⇒ TrapError(s"Function $this with args: $args was failed", Some(e)))

    override def toString: String = fnId.toString
  }

  def nameAsStr(moduleName: Option[String]): String = moduleName.getOrElse("<no-name>")

  private def nameAsStr(moduleName: String): String = nameAsStr(Option(moduleName))

}
