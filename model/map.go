package model

type WorldMap struct {
	ModelType
	MapObj   `json:"map"`
	Entities []Entity `json:"entities"`
}

// Size of a map
type Dimension struct {
	Height int
	Width  int
}

type Climate struct {
	//TODO: Come up with some base map features
}

type Props struct {
	Collision      bool `json:"collision"`      // Is it walkable?
	Speedcost      int  `json:"speedcost"`      // How much to slow ya down or speed ya up
	Damageovertime int  `json:"damageovertime"` // Does it hurt to walk on?
}

type TileFeatures struct {
	Coordinates []Cords `json:"coordinates"` // Where does this tile start?
	Properties  Props   `json:"properties"`  // Special settings
	Fill        Cords   `json:"fill"`        // Starting from coordinates fill until...

}
type Tile struct {
	TileType  string       `json:"tiletype"` // Type of tile
	TFeatures TileFeatures `json:"features"` // Details about the tile
}

// Container for the json
type MapObj struct {
	Dimensions  Dimension `json:"dimensions"` // Whats the size of the map?
	Tiles       []Tile    `json:"tiles"`      //What is the map made of?
	DefaultTile Tile
}
