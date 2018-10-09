const path = require('path');
const HtmlWebpackPlugin = require('html-webpack-plugin');
const webpack = require('webpack');
const CleanWebpackPlugin = require('clean-webpack-plugin');

module.exports = {
    entry: {
        app: ['./src/fluence.ts', './src/examples/CustomCommands.ts',
            './src/examples/IncrementAndMultiply.ts', './src/examples/DbOnPointers.ts']
    },
    devtool: 'inline-source-map',
    devServer: {
        contentBase: './bundle',
        hot: true
    },
    mode: 'development',
    module: {
        rules: [
            {
                use: 'ts-loader',
                exclude: /node_modules/
            }
        ]
    },
    resolve: {
        extensions: [ '.tsx', '.ts', '.js' ]
    },
    output: {
        filename: 'bundle.js',
        path: path.resolve(__dirname, 'bundle')
    },
    node: {
        fs: 'empty'
    },
    plugins: [
        new CleanWebpackPlugin(['bundle']),
        new HtmlWebpackPlugin(),
        new webpack.HotModuleReplacementPlugin()
    ]
};