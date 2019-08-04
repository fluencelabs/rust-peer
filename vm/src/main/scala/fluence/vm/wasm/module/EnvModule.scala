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

package fluence.vm.wasm.module

import java.lang.reflect.Modifier

import asmble.run.jvm.Module.Native
import asmble.run.jvm.ScriptContext
import cats.Monad
import cats.data.EitherT
import cats.effect.LiftIO
import fluence.vm.VmError.WasmVmError.{ApplyError, InvokeError}
import fluence.vm.VmError.{InitializationError, NoSuchFnError}
import fluence.vm.utils.safelyRunThrowable
import fluence.vm.wasm._

import scala.language.higherKinds

/**
 * Wrapper for Environment module registered by Asmble (in its term it is a Native module - please find more info
 * in WasmModule docs).
 * This module could be used for gas metering.
 *
 * @param instance a instance of Wasm Module compiled by Asmble
 * @param spentGasFunction a function returns a spent gas count
 * @param setSpentGasFunction a function sets spent gas count
 */
class EnvModule(
  private val instance: ModuleInstance,
  private val spentGasFunction: WasmFunction,
  private val setSpentGasFunction: WasmFunction
) extends WasmFunctionInvoker {

  /**
   * Returns spent gas by a Wasm modules.
   */
  def getSpentGas[F[_]: LiftIO: Monad](): EitherT[F, InvokeError, Int] =
    spentGasFunction(instance, Nil).map(_.get.intValue())

  /**
   * Clears the spent gas count.
   */
  def clearSpentGas[F[_]: LiftIO: Monad](): EitherT[F, InvokeError, Unit] =
    setSpentGasFunction(instance, Int.box(0) :: Nil).map(_ ⇒ ())

}

object EnvModule {

  /**
   * Creates instance for specified module.
   *
   * @param moduleDescription a Asmble description of the module
   * @param scriptContext a Asmble context for the module operation
   * @param spentGasFunctionName a name of the function returns a spent gas count
   * @param setSpentGasFunction a name of the function sets a spent gas count
   */
  def apply[F[_]: Monad](
    moduleDescription: Native,
    scriptContext: ScriptContext,
    spentGasFunctionName: String,
    setSpentGasFunction: String
  ): EitherT[F, ApplyError, EnvModule] =
    for {

      moduleInstance ← safelyRunThrowable(
        moduleDescription.instance(scriptContext),
        e ⇒ InitializationError(s"Unable to initialize the environment module", Some(e))
      )

      moduleMethods: Stream[WasmFunction] = moduleDescription.getCls.getDeclaredMethods.toStream
        .filter(method ⇒ Modifier.isPublic(method.getModifiers))
        .map(method ⇒ WasmFunction(method.getName, method))

      (spentGas: WasmFunction, setSpentGas: WasmFunction) <- EitherT.fromOption(
        moduleMethods
          .scanLeft((Option.empty[WasmFunction], Option.empty[WasmFunction])) {
            case (acc, m @ WasmFunction(`spentGasFunctionName`, _)) =>
              acc.copy(_1 = Some(m))
            case (acc, m @ WasmFunction(`setSpentGasFunction`, _)) =>
              acc.copy(_2 = Some(m))
            case (acc, _) =>
              acc
          }
          .collectFirst {
            case (Some(m1), Some(m2)) => (m1, m2)
          },
        NoSuchFnError(s"The env module must have function with names $spentGasFunctionName, $setSpentGasFunction"): ApplyError
      )

    } yield
      new EnvModule(
        ModuleInstance(moduleInstance),
        spentGas,
        setSpentGas
      )

}
